//! Beat, downbeat and tempo-map analysis for backing tracks.
//!
//! The command-line surface and the JSON envelope are described in `docs/output.md`; the analysis
//! itself is the beat-this model plus the post-processing in `post`.

mod analyze;
mod json;
mod models;
mod onset;
mod output;
mod post;
mod sha256;
mod stats;
mod wav;

use std::ffi::OsString;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use analyze::Model;

/// Product name, as it appears in `--version` and in the JSON envelope.
pub(crate) const NAME: &str = "opentune-beats";
/// Crate version, taken from `Cargo.toml`.
pub(crate) const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Contract version of the JSON envelope (`docs/output.md`).
pub(crate) const SCHEMA_VERSION: u32 = 1;

const EXIT_OK: u8 = 0;
const EXIT_USAGE: u8 = 2;
/// The exit code of an `analyze` run whose result cannot be written to stdout.
const EXIT_ANALYSIS_FAILURE: u8 = 5;

const USAGE: &str = "\
usage:
  opentune-beats analyze <input> [--model final|small] [--models-dir <dir>] [--pretty]
                                 [--no-refine] [--min-segment-beats <n>]
  opentune-beats version [--json]
  opentune-beats --version

--models-dir defaults to <directory of the executable>/models.
Exit codes: 0 success, 2 usage error, 3 model missing or checksum mismatch,
4 input unreadable, 5 analysis failed. The JSON contract is in docs/output.md.";

#[derive(Debug, PartialEq, Eq)]
struct AnalyzeOptions {
    input: PathBuf,
    model: Model,
    models_dir: Option<PathBuf>,
    pretty: bool,
    refine: bool,
    min_segment_beats: u32,
}

/// A rejected command line.
///
/// `detail` is the `detail` of the JSON error envelope and never contains a value the user typed.
/// `message` is written to stderr, where the full input is allowed so that the user can see what
/// was rejected (`docs/output.md`).
#[derive(Debug)]
struct UsageError {
    detail: String,
    message: String,
}

impl UsageError {
    /// A rejection that names no value the user typed.
    fn plain(detail: impl Into<String>) -> Self {
        let detail = detail.into();
        Self {
            detail: detail.clone(),
            message: detail,
        }
    }

    /// An unknown option. A plain option name (`--bogus`) is repeated in `detail`; one that
    /// carries a value, a path or non-ASCII text stays generic there and is named on stderr only.
    fn unknown_option(arg: &str) -> Self {
        let detail = if is_echoable_option(arg) {
            format!("unknown option: {arg}")
        } else {
            "unknown option".to_string()
        };
        Self {
            detail,
            message: format!("unknown option: {arg}"),
        }
    }
}

/// A rejection of a value the user typed: `detail` goes to stdout, `detail: value` to stderr.
fn value_error(detail: &str, value: &str) -> UsageError {
    UsageError {
        detail: detail.to_string(),
        message: format!("{detail}: {value}"),
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

/// True for an option name that may be repeated in the JSON `detail`: `-x` or `--long-name`,
/// ASCII letters and digits plus `-`, at most 32 characters after the dashes. Anything else (an
/// option that carries a value, a path, non-ASCII text) stays generic in stdout.
fn is_echoable_option(arg: &str) -> bool {
    let name = match arg.strip_prefix("--") {
        Some(name) => name,
        None => match arg.strip_prefix('-') {
            Some(name) => name,
            None => return false,
        },
    };
    let bytes = name.as_bytes();
    if bytes.is_empty() || bytes.len() > 32 || !bytes[0].is_ascii_alphanumeric() {
        return false;
    }
    bytes[1..]
        .iter()
        .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'-')
}

fn parse(args: &[OsString]) -> Result<Command, UsageError> {
    let first = match args.first() {
        Some(arg) => arg.to_string_lossy().into_owned(),
        None => return Err(UsageError::plain("no command given")),
    };
    match first.as_str() {
        "--version" | "-V" => {
            if args.len() > 1 {
                return Err(UsageError::plain("--version takes no arguments"));
            }
            Ok(Command::VersionLine)
        }
        "version" => parse_version(&args[1..]),
        "analyze" => parse_analyze(&args[1..]),
        "--help" | "-h" | "help" => {
            if args.len() > 1 {
                return Err(UsageError::plain("--help takes no arguments"));
            }
            Ok(Command::Help)
        }
        other => Err(value_error("unknown command", other)),
    }
}

fn parse_version(args: &[OsString]) -> Result<Command, UsageError> {
    let mut json = false;
    for raw in args {
        let arg = raw.to_string_lossy();
        if arg == "--json" {
            if json {
                return Err(UsageError::plain("--json given more than once"));
            }
            json = true;
        } else {
            return Err(value_error("unknown argument for version", &arg));
        }
    }
    if json {
        Ok(Command::VersionJson)
    } else {
        Ok(Command::VersionText)
    }
}

fn parse_analyze(args: &[OsString]) -> Result<Command, UsageError> {
    let mut input: Option<PathBuf> = None;
    let mut model = Model::Final;
    let mut models_dir: Option<PathBuf> = None;
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
                other => return Err(value_error("--model must be final or small", other)),
            }
        } else if arg == "--models-dir" {
            index += 1;
            let value = value_at(args, index, "--models-dir")?;
            if value.trim().is_empty() {
                return Err(UsageError::plain("--models-dir must not be blank"));
            }
            models_dir = Some(PathBuf::from(&args[index]));
        } else if arg == "--min-segment-beats" {
            index += 1;
            let value = value_at(args, index, "--min-segment-beats")?;
            let parsed = match value.parse::<u32>() {
                Ok(parsed) => parsed,
                Err(_) => {
                    let detail = "--min-segment-beats needs a number";
                    return Err(value_error(detail, &value));
                }
            };
            if parsed == 0 {
                let detail = "--min-segment-beats must be at least 1";
                return Err(value_error(detail, &value));
            }
            min_segment_beats = parsed;
        } else if arg.starts_with('-') && arg != "-" {
            return Err(UsageError::unknown_option(&arg));
        } else if input.is_some() {
            return Err(value_error("only one input file may be given", &arg));
        } else {
            input = Some(PathBuf::from(&args[index]));
        }
        index += 1;
    }

    let input = match input {
        Some(input) => input,
        None => return Err(UsageError::plain("analyze requires an <input> file")),
    };
    Ok(Command::Analyze(AnalyzeOptions {
        input,
        model,
        models_dir,
        pretty,
        refine,
        min_segment_beats,
    }))
}

fn value_at(args: &[OsString], index: usize, name: &str) -> Result<String, UsageError> {
    match args.get(index) {
        Some(raw) if !raw.is_empty() => Ok(raw.to_string_lossy().into_owned()),
        _ => Err(UsageError::plain(format!("{name} requires a value"))),
    }
}

/// The one line `--version` and `version` print.
fn version_line() -> String {
    format!(
        "{name} {version} (beat-this {engine})\n",
        name = NAME,
        version = VERSION,
        engine = models::BEAT_THIS_VERSION,
    )
}

fn version_json() -> String {
    let mut text = output::version_document().to_compact();
    text.push('\n');
    text
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

#[cfg(test)]
fn run<I: IntoIterator<Item = OsString>>(args: I) -> (u8, String, String) {
    let (code, stdout, stderr, _) = run_command(args);
    (code, stdout, stderr)
}

/// Runs the command. The last element is true when it was an `analyze` command (a failed parse
/// is not one), which decides the exit code when stdout cannot be written.
fn run_command<I: IntoIterator<Item = OsString>>(args: I) -> (u8, String, String, bool) {
    let args: Vec<OsString> = args.into_iter().collect();
    let command = match parse(&args) {
        Ok(command) => command,
        Err(error) => {
            let json = error_json("USAGE", &error.detail, false);
            let log = format!(
                "{name}: {message}\n\n{usage}\n",
                name = NAME,
                message = error.message,
                usage = USAGE,
            );
            return (EXIT_USAGE, json, log, false);
        }
    };
    match command {
        Command::Help => (
            EXIT_OK,
            String::new(),
            format!("{usage}\n", usage = USAGE),
            false,
        ),
        Command::VersionLine | Command::VersionText => {
            (EXIT_OK, version_line(), String::new(), false)
        }
        Command::VersionJson => (EXIT_OK, version_json(), String::new(), false),
        Command::Analyze(options) => {
            let (code, stdout, stderr) = run_analyze(&options);
            (code, stdout, stderr, true)
        }
    }
}

fn run_analyze(options: &AnalyzeOptions) -> (u8, String, String) {
    let request = analyze::Request {
        input: &options.input,
        model: options.model,
        models_dir: options.models_dir.as_deref(),
        refine: options.refine,
        min_segment_beats: options.min_segment_beats as usize,
        pretty: options.pretty,
    };
    match analyze::run(&request) {
        Ok(text) => (EXIT_OK, text, String::new()),
        Err(failure) => {
            let json = error_json(failure.code, &failure.detail, options.pretty);
            let log = format!(
                "{name}: {message}\n",
                name = NAME,
                message = failure.message
            );
            (failure.exit, json, log)
        }
    }
}

/// The exit code when stdout could not be written. Only an analysis that would have succeeded
/// becomes 5: its result was lost and the caller must know. A run that already failed keeps its
/// code, and `version`, `--version` and `--help` keep their 0 (`docs/output.md`, section 3).
fn exit_after_write_failure(code: u8, analysis: bool) -> u8 {
    if analysis && code == EXIT_OK {
        EXIT_ANALYSIS_FAILURE
    } else {
        code
    }
}

fn main() -> ExitCode {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    let (mut code, stdout, mut stderr, analysis) = run_command(args);
    if !stdout.is_empty() {
        let mut out = io::stdout().lock();
        let wrote = out.write_all(stdout.as_bytes()).is_ok() && out.flush().is_ok();
        if !wrote {
            code = exit_after_write_failure(code, analysis);
            stderr.push_str(&format!(
                "{name}: the result could not be written to stdout\n",
                name = NAME
            ));
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

    /// Runs the command with an explicit argument vector.
    fn run_argv(args: &[&str]) -> (u8, String, String) {
        let args: Vec<OsString> = args.iter().copied().map(OsString::from).collect();
        run(args)
    }

    fn version_line_text() -> String {
        format!(
            "opentune-beats {} (beat-this {})\n",
            env!("CARGO_PKG_VERSION"),
            models::BEAT_THIS_VERSION
        )
    }

    #[test]
    fn version_flag_prints_one_line() {
        let (code, stdout, stderr) = run_args("--version");
        assert_eq!(code, EXIT_OK);
        assert_eq!(stdout, version_line_text());
        assert!(stderr.is_empty());
    }

    #[test]
    fn version_json_has_the_contract_shape() {
        let (code, stdout, stderr) = run_args("version --json");
        assert_eq!(code, EXIT_OK);
        assert_eq!(stdout.lines().count(), 1);
        assert!(stdout.starts_with("{\"name\":\"opentune-beats\","));
        assert!(stdout.contains(VERSION));
        assert!(stdout.contains("\"beatThis\":\"1.1.0\""));
        assert!(stdout.contains("\"schemaVersion\":1"));
        assert!(stdout.contains("\"models\":{\"final\""));
        assert!(stdout.contains("\"mel\":{\"file\":\"mel_spectrogram.onnx\""));
        assert!(stderr.is_empty());
    }

    #[test]
    fn version_without_json_prints_the_version_line() {
        let (code, stdout, _) = run_args("version");
        assert_eq!(code, EXIT_OK);
        assert_eq!(stdout, version_line_text());
    }

    #[test]
    fn version_rejects_unknown_arguments() {
        let (code, stdout, _) = run_args("version --json --pretty");
        assert_eq!(code, EXIT_USAGE);
        assert!(stdout.contains("\"code\":\"USAGE\""));
        assert!(!stdout.contains("--pretty"));
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
        // An option that carries a value is not a plain name: stdout keeps it generic.
        let (code, stdout, _) = run_args("analyze take1.wav --bogus=1");
        assert_eq!(code, EXIT_USAGE);
        assert!(stdout.contains("\"detail\":\"unknown option\""));
        assert!(!stdout.contains("bogus"));
    }

    #[test]
    fn analyze_rejects_an_unknown_model() {
        let (code, stdout, stderr) = run_args("analyze take1.wav --model large");
        assert_eq!(code, EXIT_USAGE);
        assert!(stdout.contains("--model must be final or small"));
        assert!(!stdout.contains("large"));
        assert!(stderr.contains("large"));
    }

    #[test]
    fn analyze_rejects_an_invalid_min_segment_beats() {
        for value in ["0", "-1", "many"] {
            let line = format!("analyze take1.wav --min-segment-beats {value}");
            let (code, stdout, _) = run_args(&line);
            assert_eq!(code, EXIT_USAGE, "value {value}");
            assert!(stdout.contains("--min-segment-beats"), "value {value}");
            assert!(!stdout.contains(value), "value {value}");
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
    fn analyze_on_a_missing_file_is_an_input_error() {
        let (code, stdout, stderr) = run_args("analyze take1.wav");
        assert_eq!(code, 4);
        assert!(stdout.starts_with("{\"schemaVersion\":1,\"error\":{"));
        assert!(stdout.contains("\"code\":\"INPUT_UNREADABLE\""));
        assert!(!stdout.contains("take1"));
        assert!(stderr.contains("take1.wav"));
    }

    #[test]
    fn analyze_accepts_every_contract_option() {
        let line = concat!(
            "analyze take1.wav --model small --models-dir models --pretty --no-refine ",
            "--min-segment-beats 4",
        );
        let (code, stdout, _) = run_args(line);
        // The options parse; the file does not exist, so the run stops at the input.
        assert_eq!(code, 4);
        assert!(stdout.contains("\n  \"schemaVersion\": 1,"));
        assert!(stdout.contains("\"code\": \"INPUT_UNREADABLE\""));
        assert!(!stdout.contains("\"code\": \"USAGE\""));
    }

    #[test]
    fn input_errors_keep_the_path_out_of_the_json() {
        let (code, stdout, stderr) = run_args(r"analyze C:\Music\take1.wav");
        assert_eq!(code, 4);
        assert!(!stdout.contains("Music"));
        assert!(!stdout.contains("take1"));
        assert!(stderr.contains(r"C:\Music\take1.wav"));
    }

    /// Asserts that a rejected command line keeps the user's values out of stdout.
    fn assert_input_hidden(argv: &[&str]) {
        let (code, stdout, stderr) = run_argv(argv);
        assert_eq!(code, EXIT_USAGE, "{argv:?}");
        assert!(!stdout.contains("Music"), "{argv:?}");
        assert!(!stdout.contains("take1"), "{argv:?}");
        assert!(stderr.starts_with("opentune-beats: "), "{argv:?}");
        assert!(stderr.contains("Music"), "{argv:?}");
    }

    #[test]
    fn usage_errors_keep_user_input_out_of_the_json() {
        let path = r"C:\Music\take1.wav";
        assert_input_hidden(&["analyze", path, r"--bogus-opt=C:\Music\x"]);
        assert_input_hidden(&[path]);
        assert_input_hidden(&["version", r"C:\Music\x"]);
    }

    /// An argument that is not valid Unicode: a lone UTF-16 surrogate on Windows, an invalid
    /// UTF-8 byte on Unix. A lossy conversion would turn it into U+FFFD and name another file.
    fn non_unicode_argument() -> OsString {
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStringExt;
            OsString::from_wide(&[0x74, 0xD800, 0x2E, 0x77, 0x61, 0x76])
        }
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            OsString::from_vec(vec![b't', 0xFF, b'.', b'w', b'a', b'v'])
        }
    }

    #[test]
    fn the_input_path_keeps_its_exact_bytes() {
        let raw = non_unicode_argument();
        assert!(
            raw.to_str().is_none(),
            "the fixture must not be valid Unicode"
        );
        let args = vec![OsString::from("analyze"), raw.clone()];
        match parse(&args).expect("parses") {
            Command::Analyze(options) => assert_eq!(options.input.as_os_str(), raw.as_os_str()),
            other => panic!("expected analyze, got {other:?}"),
        }
        // `--models-dir` is a path as well and is kept the same way.
        let args = vec![
            OsString::from("analyze"),
            OsString::from("take1.wav"),
            OsString::from("--models-dir"),
            raw.clone(),
        ];
        match parse(&args).expect("parses") {
            Command::Analyze(options) => {
                let dir = options.models_dir.expect("models dir");
                assert_eq!(dir.as_os_str(), raw.as_os_str());
            }
            other => panic!("expected analyze, got {other:?}"),
        }
    }

    #[test]
    fn a_non_unicode_input_path_is_an_input_error_without_a_panic() {
        let args = vec![OsString::from("analyze"), non_unicode_argument()];
        let (code, stdout, stderr, analysis) = run_command(args);
        assert_eq!(code, 4);
        assert!(analysis);
        assert!(stdout.contains("\"code\":\"INPUT_UNREADABLE\""));
        // The path is shown lossily, and only in the stderr message: the stdout `detail` never
        // repeats a path the user typed (`docs/output.md`, section 3).
        assert!(stderr.contains("t\u{FFFD}.wav"), "stderr: {stderr}");
        assert!(!stdout.contains('\u{FFFD}'));
    }

    #[test]
    fn echoable_option_names_are_plain_ascii_names() {
        assert!(is_echoable_option("--bogus"));
        assert!(is_echoable_option("-x"));
        assert!(is_echoable_option("--a-b9"));
        assert!(!is_echoable_option("--bogus=1"));
        assert!(!is_echoable_option(r"--C:\Music"));
        assert!(!is_echoable_option("--"));
        assert!(!is_echoable_option(&format!("--{}", "a".repeat(33))));
    }

    #[test]
    fn a_lost_stdout_is_exit_five_only_for_a_successful_analysis() {
        assert_eq!(EXIT_ANALYSIS_FAILURE, 5);
        // An analysis that would have exited 0 loses its result: exit 5.
        assert_eq!(exit_after_write_failure(EXIT_OK, true), 5);
        // A run that already failed keeps its code.
        assert_eq!(exit_after_write_failure(EXIT_USAGE, true), EXIT_USAGE);
        assert_eq!(exit_after_write_failure(3, true), 3);
        assert_eq!(exit_after_write_failure(4, true), 4);
        // version, --version and --help stay 0; a usage error stays 2.
        assert_eq!(exit_after_write_failure(EXIT_OK, false), EXIT_OK);
        assert_eq!(exit_after_write_failure(EXIT_USAGE, false), EXIT_USAGE);
    }

    #[test]
    fn only_analyze_commands_are_marked_as_analysis() {
        for (line, analysis) in [
            ("--version", false),
            ("version --json", false),
            ("--help", false),
            ("analyze", false),
            ("analyze take1.wav", true),
        ] {
            let args: Vec<OsString> = line.split_whitespace().map(OsString::from).collect();
            let (_, _, _, marked) = run_command(args);
            assert_eq!(marked, analysis, "{line}");
        }
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
