//! The model files the binary expects, and where they are read from.
//!
//! The digests are the upstream ones: the mel and small models are the files committed to the
//! pinned beat-this-rs revision, and the final model is the asset of its `model-large` release
//! (see `docs/output.md`). They are compiled in, so a swapped or corrupted file is refused before
//! an inference runs.

use std::path::{Path, PathBuf};

/// The beat-this crate version the analysis pipeline is built on.
pub const BEAT_THIS_VERSION: &str = "1.1.0";

/// One model file: its short name, its file name and the bytes it must contain.
pub struct ModelFile {
    /// The short name used by `--model` and repeated in the JSON output.
    pub key: &'static str,
    pub file: &'static str,
    pub sha256: &'static str,
}

/// The log-mel front end, shared by both beat models.
pub const MEL: ModelFile = ModelFile {
    key: "mel",
    file: "mel_spectrogram.onnx",
    sha256: "fdd59e65c515331308e4c8841edf99972deca646bdf6197744c2a5b7755e3de9",
};

/// The full-accuracy beat model, from the beat-this-rs `model-large` release.
pub const FINAL: ModelFile = ModelFile {
    key: "final",
    file: "beat_this_final.onnx",
    sha256: "5f810debe53459b559127fb55bbad40035bb47cc567b20e501670f968c770f02",
};

/// The small beat model committed to the pinned beat-this-rs revision.
pub const SMALL: ModelFile = ModelFile {
    key: "small",
    file: "beat_this_small.onnx",
    sha256: "a5f8d39d989f31859454ba27afe61c5317ca95e4d9373e6853e5361b8937172f",
};

/// `--models-dir`, or `<directory of the executable>/models`.
pub fn directory(explicit: Option<&Path>) -> PathBuf {
    if let Some(dir) = explicit {
        return dir.to_path_buf();
    }
    match std::env::current_exe() {
        Ok(exe) => match exe.parent() {
            Some(parent) => parent.join("models"),
            None => PathBuf::from("models"),
        },
        Err(_) => PathBuf::from("models"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_explicit_directory_wins() {
        assert_eq!(
            directory(Some(Path::new("elsewhere"))),
            PathBuf::from("elsewhere")
        );
    }

    #[test]
    fn digests_are_64_lowercase_hex_characters() {
        for entry in [&MEL, &FINAL, &SMALL] {
            assert_eq!(entry.sha256.len(), 64);
            assert!(entry.sha256.chars().all(|ch| ch.is_ascii_hexdigit()));
            assert!(entry.file.ends_with(".onnx"));
        }
    }
}
