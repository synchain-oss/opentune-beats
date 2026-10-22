//! The analysis run: decode, verify the models, run the tracker, post-process, write JSON.

use std::io::Read;
use std::path::Path;

use beat_this::{BeatThis, RtenRuntime};

use crate::models::{self, ModelFile};
use crate::onset;
use crate::output;
use crate::post;
use crate::sha256;
use crate::wav;

/// The hop of the model's frame grid, in seconds.
pub const FRAME_SEC: f64 = 0.02;
/// How far the refinement may move a beat, in milliseconds.
pub const MAX_REFINE_MS: f64 = 30.0;
/// Below this duration there is nothing to fit a grid to.
const MIN_DURATION_SEC: f64 = 0.5;
/// The largest input file the decoder accepts, in bytes. The whole file is read into memory and
/// decoded to f32 mono, so an unbounded read is an unbounded allocation; 2 GiB is about 90
/// minutes of 48 kHz stereo 32-bit float audio (about three hours at 16 bits), far beyond a
/// backing track, and still fits the address space of a 64-bit release build with room for the
/// decoded samples. The limit is enforced on the bytes actually read, not on the reported size.
const MAX_INPUT_BYTES: u64 = 2 * 1024 * 1024 * 1024;

const EXIT_MODEL: u8 = 3;
const EXIT_INPUT: u8 = 4;
const EXIT_ANALYSIS: u8 = 5;

/// Which beat model to run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Model {
    Final,
    Small,
}

impl Model {
    pub fn file(self) -> &'static ModelFile {
        match self {
            Model::Final => &models::FINAL,
            Model::Small => &models::SMALL,
        }
    }
}

/// A failed run, in the shape the error JSON and the stderr line need.
///
/// `detail` is the `detail` of the JSON envelope: it never contains a value the user typed.
/// `message` is written to stderr, where the full input is allowed.
pub struct Failure {
    pub code: &'static str,
    pub exit: u8,
    pub detail: String,
    pub message: String,
}

impl Failure {
    fn new(code: &'static str, exit: u8, detail: impl Into<String>, message: String) -> Self {
        Self {
            code,
            exit,
            detail: detail.into(),
            message,
        }
    }
}

/// One `analyze` invocation.
pub struct Request<'a> {
    pub input: &'a Path,
    pub model: Model,
    pub models_dir: Option<&'a Path>,
    pub refine: bool,
    pub min_segment_beats: usize,
    pub pretty: bool,
}

/// Runs the pipeline and returns the JSON document (without the trailing newline).
pub fn run(request: &Request<'_>) -> Result<String, Failure> {
    let unreadable = |error: std::io::Error| {
        Failure::new(
            "INPUT_UNREADABLE",
            EXIT_INPUT,
            "the input cannot be read",
            format!("{}: {error}", request.input.display()),
        )
    };
    let too_large = |size: String| {
        Failure::new(
            "INPUT_UNREADABLE",
            EXIT_INPUT,
            "the input is larger than the supported limit",
            format!(
                "{}: {size} (limit {MAX_INPUT_BYTES} bytes)",
                request.input.display()
            ),
        )
    };
    // The file is opened once. The size the handle reports is only a fast failure: a FIFO or a
    // character device such as /dev/zero reports 0, and a file can grow while it is read. What
    // enforces the limit is the read itself, which never takes more than the limit plus one byte.
    let file = std::fs::File::open(request.input).map_err(unreadable)?;
    let declared = file.metadata().map_err(unreadable)?.len();
    if over_input_limit(declared) {
        return Err(too_large(format!("{declared} bytes")));
    }
    let bytes = read_capped(file, MAX_INPUT_BYTES)
        .map_err(unreadable)?
        .ok_or_else(|| too_large("more than the limit".to_string()))?;
    let audio = wav::decode(&bytes).map_err(|error| {
        Failure::new(
            "INPUT_UNREADABLE",
            EXIT_INPUT,
            error.detail(),
            format!("{}: {}", request.input.display(), error.detail()),
        )
    })?;
    let duration = audio.duration_sec();
    if duration < MIN_DURATION_SEC || audio.samples.len() < 8 {
        return Err(Failure::new(
            "INPUT_TOO_SHORT",
            EXIT_INPUT,
            "the input is too short to analyse",
            format!("{}: {duration:.2} s", request.input.display()),
        ));
    }

    let directory = models::directory(request.models_dir);
    let melodic = directory.join(models::MEL.file);
    let trained = directory.join(request.model.file().file);
    verify(&melodic, &models::MEL)?;
    verify(&trained, request.model.file())?;

    // The digest is computed from the file, and `BeatThis::new` then loads the model from its
    // path, so a file replaced between the two reads would be loaded unchecked. That window is
    // accepted here on purpose: beat-this 1.1 exposes models only through
    // `Runtime::load_model(&Path)` (its `RtenModel` wrapper and its input/output name map are
    // private), and loading bytes ourselves would mean reimplementing that wrapper against
    // `rten` directly. The models directory is a local, user-owned directory that the release
    // installs once; the digest still catches a truncated, corrupted or swapped download.
    let runtime = RtenRuntime;
    let mut tracker = BeatThis::new(&runtime, &melodic, &trained).map_err(|error| {
        Failure::new(
            "ANALYSIS_FAILED",
            EXIT_ANALYSIS,
            "the beat model could not be loaded",
            format!("{}: {error}", trained.display()),
        )
    })?;
    let outcome = tracker
        .analyze_audio(&audio.samples, audio.sample_rate)
        .map_err(|error| {
            Failure::new(
                "ANALYSIS_FAILED",
                EXIT_ANALYSIS,
                "beat tracking failed",
                format!("{}: {error}", request.input.display()),
            )
        })?;

    let raw: Vec<post::RawBeat> = outcome
        .beats
        .iter()
        .map(|time| post::RawBeat {
            time: f64::from(*time),
            confidence: confidence_at(&outcome.beat_logits, f64::from(*time)),
        })
        .collect();
    let downbeats: Vec<f64> = outcome
        .downbeats
        .iter()
        .map(|time| f64::from(*time))
        .collect();
    let envelope = onset::compute(&audio.samples, audio.sample_rate);
    let min_segment_beats = request.min_segment_beats.max(2);
    let settings = post::Settings {
        refine: request.refine,
        max_refine_ms: MAX_REFINE_MS,
        min_segment_beats,
        duration,
    };
    let found = raw.len();
    let analysis = post::build(raw, &downbeats, Some(&envelope), &settings).map_err(|usable| {
        Failure::new(
            "ANALYSIS_FAILED",
            EXIT_ANALYSIS,
            "the model found fewer than two usable beats",
            format!(
                "{}: {found} beat(s) from the model, {usable} usable",
                request.input.display()
            ),
        )
    })?;

    let name = file_name(request.input);
    let context = output::Document {
        model: request.model.file(),
        file_name: &name,
        duration,
        sample_rate: audio.sample_rate,
        channels: audio.channels,
        refine: request.refine,
        max_refine_ms: MAX_REFINE_MS,
        min_segment_beats,
        frame_sec: FRAME_SEC,
        analysis: &analysis,
    };
    let document = output::document(&context);
    let mut text = document.to_compact();
    if request.pretty {
        text = document.to_pretty();
    }
    text.push('\n');
    Ok(text)
}

/// The probability that the model intended a beat at `time`, from its beat logits.
fn confidence_at(logits: &[f32], time: f64) -> f64 {
    if logits.is_empty() {
        return 0.5;
    }
    let frame = (time / FRAME_SEC).round();
    let last = (logits.len() - 1) as f64;
    let index = frame.clamp(0.0, last) as usize;
    let value = f64::from(logits[index]);
    1.0 / (1.0 + (-value).exp())
}

/// Check one model file against the digest compiled into the binary.
fn verify(path: &Path, entry: &ModelFile) -> Result<(), Failure> {
    let digest = sha256::hash_file(path).map_err(|error| {
        Failure::new(
            "MODEL_MISSING",
            EXIT_MODEL,
            format!(
                "{file} is missing from the models directory",
                file = entry.file
            ),
            format!("{}: {error}", path.display()),
        )
    })?;
    if digest != entry.sha256 {
        return Err(Failure::new(
            "MODEL_CHECKSUM",
            EXIT_MODEL,
            format!(
                "{file} does not match its pinned SHA-256",
                file = entry.file
            ),
            format!("{}: sha256 {digest} != {}", path.display(), entry.sha256),
        ));
    }
    Ok(())
}

/// The last path component, converted lossily for display.
fn file_name(input: &Path) -> String {
    match input.file_name() {
        Some(name) => name.to_string_lossy().into_owned(),
        None => String::new(),
    }
}

/// Reads the whole stream, or returns `None` when it holds more than `limit` bytes. At most
/// `limit + 1` bytes are ever read, so the allocation is bounded whatever the source is.
fn read_capped<R: std::io::Read>(reader: R, limit: u64) -> std::io::Result<Option<Vec<u8>>> {
    let mut bytes = Vec::new();
    reader
        .take(limit.saturating_add(1))
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Ok(None);
    }
    Ok(Some(bytes))
}

/// True when a file of this size is past the limit the decoder accepts.
fn over_input_limit(len: u64) -> bool {
    len > MAX_INPUT_BYTES
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_name_is_the_last_component() {
        assert_eq!(file_name(Path::new("/music/take1.wav")), "take1.wav");
        assert_eq!(file_name(Path::new("take1.wav")), "take1.wav");
        #[cfg(windows)]
        assert_eq!(file_name(Path::new(r"C:\Music\take1.wav")), "take1.wav");
    }

    #[test]
    fn file_name_drops_the_directory_and_converts_lossily_only_for_display() {
        // A path without a final component has no name; the directory must not leak instead.
        assert_eq!(file_name(Path::new("/")), "");
        assert_eq!(file_name(Path::new("/music/..")), "");
        #[cfg(unix)]
        {
            use std::ffi::OsStr;
            use std::os::unix::ffi::OsStrExt;
            let path = Path::new(OsStr::from_bytes(b"/m\xFFusic/t\xFFake.wav"));
            assert_eq!(file_name(path), "t\u{FFFD}ake.wav");
        }
        #[cfg(windows)]
        {
            use std::ffi::OsString;
            use std::os::windows::ffi::OsStringExt;
            let name = OsString::from_wide(&[0x74, 0xD800, 0x2E, 0x77, 0x61, 0x76]);
            let path = Path::new(r"C:\Music").join(name);
            assert_eq!(file_name(&path), "t\u{FFFD}.wav");
        }
    }

    #[test]
    fn a_missing_input_is_an_input_error() {
        let request = Request {
            input: Path::new("definitely-not-here.wav"),
            model: Model::Final,
            models_dir: None,
            refine: true,
            min_segment_beats: 8,
            pretty: false,
        };
        let failure = run(&request).expect_err("fails");
        assert_eq!(failure.code, "INPUT_UNREADABLE");
        assert_eq!(failure.exit, EXIT_INPUT);
        assert!(!failure.detail.contains("definitely-not-here"));
        assert!(failure.message.contains("definitely-not-here"));
    }

    #[test]
    fn confidence_is_a_sigmoid_of_the_logit() {
        assert!((confidence_at(&[0.0], 0.0) - 0.5).abs() < 1e-9);
        assert!(confidence_at(&[8.0], 0.0) > 0.99);
        assert!(confidence_at(&[-8.0], 0.0) < 0.01);
        assert!((confidence_at(&[], 1.0) - 0.5).abs() < 1e-9);
    }

    #[test]
    fn an_input_past_the_size_limit_is_refused() {
        assert!(!over_input_limit(0));
        assert!(!over_input_limit(MAX_INPUT_BYTES));
        assert!(over_input_limit(MAX_INPUT_BYTES + 1));
    }

    #[test]
    fn a_stream_past_the_limit_is_refused_without_reading_it_all() {
        // An endless source (what /dev/zero looks like) reports no size; the read must stop.
        let endless = std::io::repeat(0);
        assert!(read_capped(endless, 1000).expect("reads").is_none());
        // Exactly the limit is accepted, one byte more is not.
        let exact = vec![7u8; 1000];
        assert_eq!(
            read_capped(&exact[..], 1000).expect("reads"),
            Some(exact.clone())
        );
        let over = vec![7u8; 1001];
        assert!(read_capped(&over[..], 1000).expect("reads").is_none());
        assert_eq!(read_capped(&[][..], 1000).expect("reads"), Some(Vec::new()));
    }
}
