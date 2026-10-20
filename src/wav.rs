//! WAV decoding.
//!
//! The contract promises PCM 16/24/32 and IEEE float 32, any sample rate, mono or stereo; other
//! channel counts are averaged down as well. The decoder is deliberately strict: a malformed
//! header is an input error, not something to guess around.

/// Decoded audio, downmixed to mono.
pub struct Audio {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
    pub channels: u16,
}

impl Audio {
    pub fn duration_sec(&self) -> f64 {
        if self.sample_rate == 0 {
            return 0.0;
        }
        self.samples.len() as f64 / self.sample_rate as f64
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum WavError {
    NotWave,
    Truncated,
    Unsupported(&'static str),
}

impl WavError {
    /// A sentence for the error JSON; it never names the file.
    pub fn detail(&self) -> &'static str {
        match self {
            WavError::NotWave => "the input is not a RIFF/WAVE file",
            WavError::Truncated => "the WAV file is truncated",
            WavError::Unsupported(reason) => reason,
        }
    }
}

struct Format {
    code: u16,
    channels: u16,
    sample_rate: u32,
    bits: u16,
}

/// The `data` size a streaming writer leaves behind when it did not know the length up front.
const STREAMED_CHUNK_SIZE: u32 = 0xFFFF_FFFF;

/// The reason an RF64/BW64 file is refused: it is a real WAV family, not a broken RIFF file.
const RF64_REASON: &str = "RF64/BW64 WAV files are not supported; write a plain RIFF/WAVE file";

pub fn decode(bytes: &[u8]) -> Result<Audio, WavError> {
    if bytes.len() < 12 {
        return Err(WavError::NotWave);
    }
    if (&bytes[0..4] == b"RF64" || &bytes[0..4] == b"BW64") && &bytes[8..12] == b"WAVE" {
        return Err(WavError::Unsupported(RF64_REASON));
    }
    if &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(WavError::NotWave);
    }
    let mut format: Option<Format> = None;
    let mut data: Option<&[u8]> = None;
    let mut position = 12;
    while position + 8 <= bytes.len() {
        let id = &bytes[position..position + 4];
        let declared = le32(&bytes[position + 4..position + 8]) as usize;
        let body_start = position + 8;
        // A writer that streams a WAV cannot know the data size when it writes the header and
        // fills the field with 0xFFFFFFFF; the samples then run to the end of the file.
        let streamed = id == b"data" && declared == STREAMED_CHUNK_SIZE as usize;
        let body_end = if streamed {
            bytes.len()
        } else {
            body_start
                .checked_add(declared)
                .ok_or(WavError::Truncated)?
        };
        if body_end > bytes.len() {
            return Err(WavError::Truncated);
        }
        let body = &bytes[body_start..body_end];
        if id == b"fmt " {
            format = Some(parse_format(body)?);
        } else if id == b"data" {
            data = Some(body);
        }
        position = body_end + if streamed { 0 } else { declared & 1 };
    }
    let format = format.ok_or(WavError::Truncated)?;
    let data = data.ok_or(WavError::Truncated)?;
    decode_samples(&format, data)
}

fn parse_format(body: &[u8]) -> Result<Format, WavError> {
    if body.len() < 16 {
        return Err(WavError::Truncated);
    }
    let mut code = le16(&body[0..2]);
    let channels = le16(&body[2..4]);
    let sample_rate = le32(&body[4..8]);
    let bits = le16(&body[14..16]);
    if code == 0xfffe {
        // WAVE_FORMAT_EXTENSIBLE: the real format is the first word of the subformat GUID.
        if body.len() < 26 {
            return Err(WavError::Truncated);
        }
        code = le16(&body[24..26]);
    }
    if channels == 0 || channels > 8 {
        return Err(WavError::Unsupported(
            "the WAV file has an unsupported channel count",
        ));
    }
    if sample_rate == 0 {
        return Err(WavError::Unsupported(
            "the WAV file has a sample rate of zero",
        ));
    }
    Ok(Format {
        code,
        channels,
        sample_rate,
        bits,
    })
}

fn decode_samples(format: &Format, data: &[u8]) -> Result<Audio, WavError> {
    let bytes_per_sample = match (format.code, format.bits) {
        (1, 8) | (1, 16) | (1, 24) | (1, 32) | (3, 32) => (format.bits / 8) as usize,
        _ => {
            return Err(WavError::Unsupported(
                "the WAV file uses an unsupported sample format",
            ))
        }
    };
    let frame_bytes = bytes_per_sample * format.channels as usize;
    if frame_bytes == 0 {
        return Err(WavError::Unsupported(
            "the WAV file uses an unsupported sample format",
        ));
    }
    let frames = data.len() / frame_bytes;
    let mut samples = Vec::with_capacity(frames);
    for frame in data.chunks_exact(frame_bytes) {
        let mut sum = 0.0f32;
        for channel in 0..format.channels as usize {
            let start = channel * bytes_per_sample;
            let value = match (format.code, format.bits) {
                (1, 8) => (frame[start] as f32 - 128.0) / 128.0,
                (1, 16) => i16::from_le_bytes([frame[start], frame[start + 1]]) as f32 / 32_768.0,
                (1, 24) => {
                    let value =
                        i32::from_le_bytes([0, frame[start], frame[start + 1], frame[start + 2]]);
                    (value >> 8) as f32 / 8_388_608.0
                }
                (1, 32) => {
                    let value = i32::from_le_bytes([
                        frame[start],
                        frame[start + 1],
                        frame[start + 2],
                        frame[start + 3],
                    ]);
                    value as f32 / 2_147_483_648.0
                }
                _ => {
                    let value = f32::from_le_bytes([
                        frame[start],
                        frame[start + 1],
                        frame[start + 2],
                        frame[start + 3],
                    ]);
                    if value.is_finite() {
                        value
                    } else {
                        0.0
                    }
                }
            };
            sum += value;
        }
        samples.push(sum / format.channels as f32);
    }
    Ok(Audio {
        samples,
        sample_rate: format.sample_rate,
        channels: format.channels,
    })
}

fn le16(bytes: &[u8]) -> u16 {
    u16::from_le_bytes([bytes[0], bytes[1]])
}

fn le32(bytes: &[u8]) -> u32 {
    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wav(code: u16, channels: u16, rate: u32, bits: u16, data: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
        out.extend_from_slice(b"WAVE");
        out.extend_from_slice(b"fmt ");
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(&code.to_le_bytes());
        out.extend_from_slice(&channels.to_le_bytes());
        out.extend_from_slice(&rate.to_le_bytes());
        let block = channels as u32 * (bits as u32 / 8);
        out.extend_from_slice(&(rate * block).to_le_bytes());
        out.extend_from_slice(&(block as u16).to_le_bytes());
        out.extend_from_slice(&bits.to_le_bytes());
        out.extend_from_slice(b"data");
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(data);
        out
    }

    #[test]
    fn stereo_16_bit_is_averaged_to_mono() {
        let mut data = Vec::new();
        data.extend_from_slice(&16_384i16.to_le_bytes());
        data.extend_from_slice(&(-16_384i16).to_le_bytes());
        let audio = decode(&wav(1, 2, 44_100, 16, &data)).expect("decodes");
        assert_eq!(audio.sample_rate, 44_100);
        assert_eq!(audio.channels, 2);
        assert_eq!(audio.samples.len(), 1);
        assert!(audio.samples[0].abs() < 1e-9);
        assert!((audio.duration_sec() - 1.0 / 44_100.0).abs() < 1e-12);
    }

    #[test]
    fn float32_is_kept() {
        let mut data = Vec::new();
        data.extend_from_slice(&0.25f32.to_le_bytes());
        data.extend_from_slice(&(-0.5f32).to_le_bytes());
        let audio = decode(&wav(3, 1, 48_000, 32, &data)).expect("decodes");
        assert_eq!(audio.channels, 1);
        assert_eq!(audio.samples, vec![0.25, -0.5]);
    }

    #[test]
    fn twenty_four_bit_is_sign_extended() {
        let mut data = Vec::new();
        data.extend_from_slice(&[0x00, 0x00, 0x40]); // 0.5 in 24-bit
        let audio = decode(&wav(1, 1, 44_100, 24, &data)).expect("decodes");
        assert!((audio.samples[0] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn a_bad_magic_is_not_a_wave_file() {
        assert!(matches!(decode(b"not a wave file"), Err(WavError::NotWave)));
    }

    #[test]
    fn a_truncated_chunk_is_reported() {
        let mut bytes = wav(1, 1, 44_100, 16, &[0, 0]);
        bytes.truncate(30);
        assert!(matches!(decode(&bytes), Err(WavError::Truncated)));
    }

    #[test]
    fn a_streamed_data_chunk_runs_to_the_end() {
        // A writer that streams cannot know the data size and leaves 0xFFFFFFFF behind.
        let samples: Vec<u8> = [0.25f32, -0.5]
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect();
        let mut bytes = wav(3, 1, 44_100, 32, &samples);
        let data_size = 36 + 4;
        bytes[data_size..data_size + 4].copy_from_slice(&STREAMED_CHUNK_SIZE.to_le_bytes());
        let audio = decode(&bytes).expect("decodes");
        assert_eq!(audio.samples, vec![0.25, -0.5]);
    }

    #[test]
    fn rf64_is_refused_with_a_clear_reason() {
        let mut bytes = wav(1, 1, 44_100, 16, &[0, 0]);
        bytes[0..4].copy_from_slice(b"RF64");
        let error = decode(&bytes).err().expect("fails");
        assert_eq!(error, WavError::Unsupported(RF64_REASON));
        assert!(error.detail().contains("RF64"));
    }

    #[test]
    fn an_unknown_sample_format_is_rejected() {
        let error = decode(&wav(6, 1, 8_000, 8, &[0])).err().expect("fails");
        let reason = "the WAV file uses an unsupported sample format";
        assert_eq!(error, WavError::Unsupported(reason));
    }
}
