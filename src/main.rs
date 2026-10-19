//! Beat, downbeat and tempo-map analysis for backing tracks.
//!
//! This is the first, deliberately small build: the command-line surface and the JSON envelope
//! described in `docs/output.md` exist, but the beat tracker is not linked yet, so `analyze`
//! reports `ANALYSIS_FAILED` instead of a beat grid.

use std::ffi::OsString;
use std::io::{self, Write};
use std::process::ExitCode;

/// Product name, as it appears in `--version` and in the JSON envelope.
const NAME: &str = "opentune-beats";
/// Crate version, taken from `Cargo.toml`.
const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Contract version of the JSON envelope (`docs/output.md`).
const SCHEMA_VERSION: u32 = 1;

const EXIT_OK: u8 = 0;
const EXIT_USAGE: u8 = 2;
const EXIT_ANALYSIS_FAILED: u8 = 5;

const USAGE: &str = "\
usage:
  opentune-beats analyze <input> [--model final|small] [--models-dir <dir>] [--pretty]
                                 [--no-refine] [--min-segment-beats <n>]
  opentune-beats version [--json]
  opentune-beats --version

Exit codes: 0 success, 2 usage error, 3 model missing or checksum mismatch,
4 input unreadable, 5 analysis failed. The JSON contract is in docs/output.md.";

#[derive(Debug, PartialEq, Eq)]
enum Model {
    Final,
    Small,
}

impl Model {
    fn as_str(&self) -> &'static str {
        match self {
            Model::Final => "final",
            Model::Small => "small",
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
struct AnalyzeOptions {
    input: String,
    model: Model,
    pretty: bool,
    refine: bool,
    min_segment_beats: u32,
}

impl AnalyzeOptions {
    /// The body of the `ANALYSIS_FAILED` error. Only the file name of the input is echoed: the
    /// JSON output must never carry the input's directory (`docs/output.md`).
    fn not_implemented_detail(&self) -> String {
        let refine = if self.refine { "on" } else { "off" };
        format!(
            "beat tracking is not implemented yet (file {}, model {}, refine {}, min beats {})",
            file_name(&self.input),
            self.model.as_str(),
            refine,
            self.min_segment_beats,
        )
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Command {
    Help,
    VersionLine,
    VersionText,
    VersionJson,
    Analyze(AnalyzeOptions),
}

/// The last path component, splitting on both separators so that the result is the same on every
/// platform the binary is built for.
fn file_name(input: &str) -> &str {
    input.rsplit(['/', '\\']).next().unwrap_or(input)
}

fn parse(args: &[OsString]) -> Result<Command, String> {
    let first = match args.first() {
        Some(arg) => arg.to_string_lossy().into_owned(),
        None => return Err("no command given".to_string()),
    };
    match first.as_str() {
        "--version" | "-V" => {
            if args.len() > 1 {
                return Err("--version takes no arguments".to_string());
            }
            Ok(Command::VersionLine)
        }
        "version" => parse_version(&args[1..]),
        "analyze" => parse_analyze(&args[1..]),
        "--help" | "-h" | "help" => {
            if args.len() > 1 {
                return Err("--help takes no arguments".to_string());
            }
            Ok(Command::Help)
        }
        other => Err(format!("unknown command: {other}")),
    }
}

fn parse_version(args: &[OsString]) -> Result<Command, String> {
    let mut json = false;
    for raw in args {
        let arg = raw.to_string_lossy();
        if arg == "--json" {
            if json {
                return Err("--json given more than once".to_string());
            }
            json = true;
        } else {
            return Err(format!("unknown argument for version: {arg}"));
        }
    }
    if json {
        Ok(Command::VersionJson)
    } else {
        Ok(Command::VersionText)
    }
}

fn parse_analyze(args: &[OsString]) -> Result<Command, String> {
    let mut input: Option<String> = None;
    let mut model = Model::Final;
    let mut pretty = false;
    let mut refine = true;
    let mut min_segment_beats: u32 = 8;

    let mut index = 0;
    while index < args.len() {
        let arg = args[index].to_string_lossy().into_owned();
        if arg == "--pretty" {
            pretty = true;
        } else if arg == "--no-refine" {
            refine = false;
        } else if arg == "--model" {
            index += 1;
            let value = value_at(args, index, "--model")?;
            match value.as_str() {
                "final" => model = Model::Final,
                "small" => model = Model::Small,
                other => return Err(format!("--model must be final or small, got {other}")),
            }
        } else if arg == "--models-dir" {
            index += 1;
            // Only validated here; the model loader reads the directory once the engine lands.
            let value = value_at(args, index, "--models-dir")?;
            if value.trim().is_empty() {
                return Err("--models-dir must not be blank".to_string());
            }
        } else if arg == "--min-segment-beats" {
            index += 1;
            let value = value_at(args, index, "--min-segment-beats")?;
            let parsed = match value.parse::<u32>() {
                Ok(parsed) => parsed,
                Err(_) => return Err(format!("--min-segment-beats needs a number, got {value}")),
            };
            if parsed == 0 {
                return Err("--min-segment-beats must be at least 1".to_string());
            }
            min_segment_beats = parsed;
        } else if arg.starts_with('-') && arg != "-" {
            return Err(format!("unknown option: {arg}"));
        } else if input.is_some() {
            return Err("only one input file may be given".to_string());
        } else {
            input = Some(arg);
        }
        index += 1;
    }

    let input = match input {
        Some(input) => input,
        None => return Err("analyze requires an <input> file".to_string()),
    };
    Ok(Command::Analyze(AnalyzeOptions {
        input,
        model,
        pretty,
        refine,
        min_segment_beats,
    }))
}

fn value_at(args: &[OsString], index: usize, name: &str) -> Result<String, String> {
    match args.get(index) {
        Some(raw) if !raw.is_empty() => Ok(raw.to_string_lossy().into_owned()),
        _ => Err(format!("{name} requires a value")),
    }
}

/// The one line `--version` and `version` print. The `(beat-this <version>)` suffix of the
/// contract is added together with the engine dependency; see `docs/output.md`.
fn version_line() -> String {
    format!("{name} {version}\n", name = NAME, version = VERSION)
}

fn version_json() -> String {
    let schema = SCHEMA_VERSION.to_string();
    let mut out = String::from("{\"name\":\"");
    out.push_str(NAME);
    out.push_str("\",\"version\":\"");
    out.push_str(VERSION);
    out.push_str("\",\"beatThis\":null,\"schemaVersion\":");
    out.push_str(&schema);
    out.push_str(",\"models\":{}}\n");
    out
}

/// The one JSON object a failed run writes to stdout. It is assembled from short pieces so that
/// no source line holds a JSON literal that is hard to review.
fn error_json(code: &str, detail: &str, pretty: bool) -> String {
    let detail = json_escape(detail);
    let schema = SCHEMA_VERSION.to_string();
    let mut out = String::new();
    if pretty {
        out.push_str("{\n  \"schemaVersion\": ");
        out.push_str(&schema);
        out.push_str(",\n  \"error\": {\n    \"code\": \"");
        out.push_str(code);
        out.push_str("\",\n    \"detail\": \"");
        out.push_str(&detail);
        out.push_str("\"\n  }\n}\n");
    } else {
        out.push_str("{\"schemaVersion\":");
        out.push_str(&schema);
        out.push_str(",\"error\":{\"code\":\"");
        out.push_str(code);
        out.push_str("\",\"detail\":\"");
        out.push_str(&detail);
        out.push_str("\"}}\n");
    }
    out
}

fn json_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

fn run<I: IntoIterator<Item = OsString>>(args: I) -> (u8, String, String) {
    let args: Vec<OsString> = args.into_iter().collect();
    let command = match parse(&args) {
        Ok(command) => command,
        Err(message) => {
            let json = error_json("USAGE", &message, false);
            let log = format!("{name}: {message}\n\n{usage}\n", name = NAME, usage = USAGE);
            return (EXIT_USAGE, json, log);
        }
    };
    match command {
        Command::Help => (EXIT_OK, String::new(), format!("{usage}\n", usage = USAGE)),
        Command::VersionLine | Command::VersionText => (EXIT_OK, version_line(), String::new()),
        Command::VersionJson => (EXIT_OK, version_json(), String::new()),
        Command::Analyze(options) => analyze(&options),
    }
}

fn analyze(options: &AnalyzeOptions) -> (u8, String, String) {
    let detail = options.not_implemented_detail();
    let json = error_json("ANALYSIS_FAILED", &detail, options.pretty);
    (EXIT_ANALYSIS_FAILED, json, String::new())
}

fn main() -> ExitCode {
    let (code, stdout, stderr) = run(std::env::args_os().skip(1));
    if !stdout.is_empty() {
        let mut out = io::stdout().lock();
        let wrote = out.write_all(stdout.as_bytes()).is_ok() && out.flush().is_ok();
        if !wrote {
            return ExitCode::from(EXIT_ANALYSIS_FAILED);
        }
    }
    if !stderr.is_empty() {
        let mut err = io::stderr().lock();
        let _ = err.write_all(stderr.as_bytes());
    }
    ExitCode::from(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs the command with a whitespace-separated command line.
    fn run_args(line: &str) -> (u8, String, String) {
        let args: Vec<OsString> = line.split_whitespace().map(OsString::from).collect();
        run(args)
    }

    #[test]
    fn version_flag_prints_one_line() {
        let (code, stdout, stderr) = run_args("--version");
        assert_eq!(code, EXIT_OK);
        assert_eq!(stdout, format!("opentune-beats {}\n", env!("CARGO_PKG_VERSION")));
        assert!(stderr.is_empty());
    }

    #[test]
    fn version_json_has_the_contract_shape() {
        let (code, stdout, stderr) = run_args("version --json");
        assert_eq!(code, EXIT_OK);
        assert_eq!(stdout.lines().count(), 1);
        assert!(stdout.starts_with("{\"name\":\"opentune-beats\","));
        assert!(stdout.contains(VERSION));
        assert!(stdout.contains("\"beatThis\":null"));
        assert!(stdout.contains("\"schemaVersion\":1"));
        assert!(stdout.ends_with(",\"models\":{}}\n"));
        assert!(stderr.is_empty());
    }

    #[test]
    fn version_without_json_prints_the_version_line() {
        let (code, stdout, _) = run_args("version");
        assert_eq!(code, EXIT_OK);
        assert_eq!(stdout, format!("opentune-beats {}\n", env!("CARGO_PKG_VERSION")));
    }

    #[test]
    fn version_rejects_unknown_arguments() {
        let (code, stdout, _) = run_args("version --json --pretty");
        assert_eq!(code, EXIT_USAGE);
        assert!(stdout.contains("\"code\":\"USAGE\""));
    }

    #[test]
    fn version_flag_rejects_extra_arguments() {
        let (code, stdout, _) = run_args("--version analyze");
        assert_eq!(code, EXIT_USAGE);
        assert!(stdout.contains("\"code\":\"USAGE\""));
    }

    #[test]
    fn no_arguments_is_a_usage_error() {
        let (code, stdout, stderr) = run_args("");
        assert_eq!(code, EXIT_USAGE);
        assert!(stdout.starts_with("{\"schemaVersion\":1,\"error\":{"));
        assert!(stdout.contains("\"code\":\"USAGE\""));
        assert!(stderr.contains("usage:"));
    }

    #[test]
    fn help_prints_usage_to_stderr_and_exits_zero() {
        let (code, stdout, stderr) = run_args("--help");
        assert_eq!(code, EXIT_OK);
        assert!(stdout.is_empty());
        assert!(stderr.contains("usage:"));
        assert!(stderr.contains("opentune-beats analyze"));
    }

    #[test]
    fn analyze_without_input_is_a_usage_error() {
        let (code, stdout, _) = run_args("analyze");
        assert_eq!(code, EXIT_USAGE);
        assert!(stdout.contains("\"code\":\"USAGE\""));
    }

    #[test]
    fn analyze_rejects_unknown_options() {
        let (code, stdout, _) = run_args("analyze take1.wav --bogus");
        assert_eq!(code, EXIT_USAGE);
        assert!(stdout.contains("unknown option: --bogus"));
    }

    #[test]
    fn analyze_rejects_an_unknown_model() {
        let (code, stdout, _) = run_args("analyze take1.wav --model large");
        assert_eq!(code, EXIT_USAGE);
        assert!(stdout.contains("--model must be final or small"));
    }

    #[test]
    fn analyze_rejects_an_invalid_min_segment_beats() {
        for value in ["0", "-1", "many"] {
            let line = format!("analyze take1.wav --min-segment-beats {value}");
            let (code, stdout, _) = run_args(&line);
            assert_eq!(code, EXIT_USAGE, "value {value}");
            assert!(stdout.contains("--min-segment-beats"), "value {value}");
        }
    }

    #[test]
    fn analyze_requires_a_value_for_each_option() {
        for option in ["--model", "--models-dir", "--min-segment-beats"] {
            let line = format!("analyze take1.wav {option}");
            let (code, stdout, _) = run_args(&line);
            assert_eq!(code, EXIT_USAGE, "option {option}");
            assert!(stdout.contains(option), "option {option}");
        }
    }

    #[test]
    fn analyze_reports_analysis_failed_until_the_engine_lands() {
        let (code, stdout, stderr) = run_args("analyze take1.wav");
        assert_eq!(code, EXIT_ANALYSIS_FAILED);
        assert!(stdout.starts_with("{\"schemaVersion\":1,\"error\":{"));
        assert!(stdout.contains("\"code\":\"ANALYSIS_FAILED\""));
        assert!(stdout.contains("take1.wav"));
        assert_eq!(stdout.lines().count(), 1);
        assert!(stderr.is_empty());
    }

    #[test]
    fn analyze_accepts_every_contract_option() {
        let line = concat!(
            "analyze take1.wav --model small --models-dir models --pretty --no-refine ",
            "--min-segment-beats 4",
        );
        let (code, stdout, _) = run_args(line);
        assert_eq!(code, EXIT_ANALYSIS_FAILED);
        assert!(stdout.contains("\n  \"schemaVersion\": 1,"));
        assert!(stdout.contains("\"code\": \"ANALYSIS_FAILED\""));
        assert!(stdout.contains("model small"));
        assert!(stdout.contains("refine off"));
        assert!(stdout.contains("min beats 4"));
    }

    #[test]
    fn analysis_error_never_echoes_the_directory() {
        let (_, stdout, _) = run_args(r"analyze C:\Music\take1.wav");
        assert!(stdout.contains("take1.wav"));
        assert!(!stdout.contains("Music"));
    }

    #[test]
    fn file_name_splits_both_separators() {
        assert_eq!(file_name(r"C:\Music\take1.wav"), "take1.wav");
        assert_eq!(file_name("/music/take1.wav"), "take1.wav");
        assert_eq!(file_name("take1.wav"), "take1.wav");
    }

    #[test]
    fn json_escape_quotes_backslashes_and_controls() {
        assert_eq!(json_escape("a\"b\\c\nd\te"), "a\\\"b\\\\c\\nd\\te");
        assert_eq!(json_escape("\u{1}"), "\\u0001");
    }

    #[test]
    fn error_json_has_the_contract_shape() {
        let compact = error_json("USAGE", "missing <input>", false);
        let expected = concat!(
            "{\"schemaVersion\":1,\"error\":{\"code\":\"USAGE\",",
            "\"detail\":\"missing <input>\"}}\n",
        );
        assert_eq!(compact, expected);
        assert_eq!(compact.lines().count(), 1);
        let pretty = error_json("USAGE", "missing <input>", true);
        assert!(pretty.starts_with("{\n  \"schemaVersion\": 1,\n  \"error\": {\n"));
        assert!(pretty.contains("\"code\": \"USAGE\""));
        assert!(pretty.ends_with("\n  }\n}\n"));
    }
}
