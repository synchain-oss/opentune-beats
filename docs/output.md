# Output contract

The command-line interface and the JSON envelope below are the contract between this tool and
its callers. The `beat-grid` skill of [OpenTune MCP Server](https://github.com/synchain-oss/opentune-mcp-server)
is the first caller; anything else that runs the binary is held to the same rules.

Rules that apply to every change here:

- **`schemaVersion` only increases on an incompatible change.** Fields are only added, never
  renamed or removed, and the meaning and the unit of an existing field never change.
- **Warning and error codes are only added.** A reader must accept a code it does not know.
- **Times are seconds** from the start of the input file and are written with 4 decimal places.
  A field whose name ends in `Ms` is in milliseconds.
- **`analyze` and `version --json` write exactly one JSON object to stdout.** Logs and progress
  go to stderr. A failure, including a usage error, still writes one JSON object to stdout, so a
  caller parses stdout the same way whether the run succeeded or not. `version` and `--version`
  print one line of text; `--help` writes only to stderr (section 1).

## 1. Commands

```
opentune-beats analyze <input> [--model final|small] [--models-dir <dir>] [--pretty]
                               [--no-refine] [--min-segment-beats <n>]
opentune-beats version [--json]
opentune-beats --version
```

| Command | Output |
| --- | --- |
| `analyze <input>` | The analysis JSON of section 2, or the error JSON of section 3 |
| `version` | One line: `opentune-beats <version> (beat-this <crate version>)`; this build prints the line without the `beat-this` suffix until the engine is linked (section 6) |
| `version --json` | The version JSON of section 4 |
| `--version` | The same one line as `version` |
| `--help`, `-h`, `help` | The usage text on stderr; stdout stays empty; exit code 0 |

`analyze` options:

| Option | Default | Meaning |
| --- | --- | --- |
| `<input>` | – | A WAV file (PCM 16/24/32-bit or float32, any sample rate, mono or stereo). Other formats that the decoder handles are supported on a best-effort basis; only WAV is promised. |
| `--model final\|small` | `final` | Which beat model to run. `final` is about 80 MB and more accurate; `small` is about 10 MB. |
| `--models-dir <dir>` | `<directory of the executable>/models` | Where the model files are read from. |
| `--pretty` | off | Indent the JSON output. |
| `--no-refine` | off | Skip the millisecond refinement against the onset envelope. |
| `--min-segment-beats <n>` | `8` | A constant-tempo segment shorter than this many beats is merged into its neighbour. |

`--models-dir` must contain `mel_spectrogram.onnx` and, depending on `--model`,
`beat_this_final.onnx` or `beat_this_small.onnx`. The SHA-256 of each file is compiled into the
binary and checked before the model is loaded; a mismatch is exit code 3 with `MODEL_CHECKSUM`,
a missing file is exit code 3 with `MODEL_MISSING`. Nothing is downloaded at run time.

`--version` and `version` never load a model, so they work in a directory with no models and are
the self-check a caller runs after installing the binary.

## 2. Analysis JSON

```jsonc
{
  "schemaVersion": 1,
  "tool": { "name": "opentune-beats", "version": "0.1.0", "beatThis": "1.1.0" },
  "model": { "name": "final", "file": "beat_this_final.onnx", "sha256": "…", "melSha256": "…" },
  "input": { "fileName": "take1.wav", "durationSec": 0.0, "sampleRateHz": 44100, "channels": 2 },
  "params": { "refine": true, "maxRefineMs": 30, "minSegmentBeats": 8, "frameSec": 0.02 },
  "beats": [
    { "index": 0, "timeSec": 0.0, "rawTimeSec": 0.0, "confidence": 0.0,
      "isDownbeat": false, "bar": 1, "beatInBar": 1, "segment": 0 }
  ],
  "segments": [
    { "index": 0, "startSec": 0.0, "endSec": 0.0, "firstBeat": 0, "lastBeat": 0,
      "bpm": 0.0, "beatPeriodSec": 0.0,
      "timeSignature": { "numerator": 4, "denominator": 4 },
      "timeSignatureCandidates": [ { "numerator": 4, "denominator": 4, "score": 0.0 } ],
      "confidence": 0.0, "fitResidualMs": { "median": 0.0, "p95": 0.0 },
      "octaveNote": null }
  ],
  "tempoChanges": [ { "atSec": 0.0, "fromBpm": 0.0, "toBpm": 0.0, "kind": "step" } ],
  "refinement": { "applied": true, "globalOffsetMs": 0.0, "medianAdjustMs": 0.0, "p95AdjustMs": 0.0 },
  "summary": { "beats": 0, "downbeats": 0, "segments": 0, "dominantBpm": 0.0, "lowConfidenceSegments": [] },
  "warnings": [ { "code": "LOW_CONFIDENCE_SEGMENT", "detail": "…", "segment": 0 } ]
}
```

- `input.fileName` is the file name only, never the directory: the output must not leak where the
  file lives on the caller's machine. `input.durationSec`, `sampleRateHz` and `channels` describe
  the decoded audio.
- `params` echoes the parameters the run actually used, so a stored result can be reproduced.
  `maxRefineMs` is the window the refinement may move a beat; `frameSec` is the hop of the
  onset envelope.
- `beats[].timeSec` is the final time, `beats[].rawTimeSec` the model's time before refinement.
  `confidence` is 0..1. `index` counts from 0 in time order. `segment` is the index into
  `segments`.
- `bar` and `beatInBar` count from 1 and are `null` when there is no reliable downbeat; a
  `NO_DOWNBEATS` warning says so. `isDownbeat` is then `false` for every beat.
- `segments` are the constant-tempo spans: they cover the whole input, are contiguous and do not
  overlap. `startSec` and `endSec` are the span, `firstBeat` and `lastBeat` the beat indices in it.
  `bpm` is the tempo and `beatPeriodSec` its reciprocal in seconds. `timeSignature` is the best
  reading and `timeSignatureCandidates` the scored alternatives. `fitResidualMs` describes how
  well a constant grid fits the beats in the span.
- `segments[].octaveNote` is `"halved"`, `"doubled"` or `null`: set when a segment's reading was
  reconciled with the neighbouring ones because the tempo was a factor of two off.
- `tempoChanges` lists the boundaries between segments: `atSec` is the boundary, `fromBpm` and
  `toBpm` the tempos on either side, `kind` is `"step"` for an abrupt change and `"ramp"` for a
  gradual one.
- `refinement.applied` is `false` when `--no-refine` was given or the refinement could not run
  (a `REFINE_SKIPPED` warning then explains why). `globalOffsetMs` is the offset applied to every
  beat; `medianAdjustMs` and `p95AdjustMs` describe the per-beat adjustments.
- `summary` is the aggregate a caller can show without walking the arrays.
  `lowConfidenceSegments` lists the indices of the segments whose confidence is below the
  threshold, in ascending order.
- `warnings` is ordered by the position it refers to. `segment` is the segment index, or `null`
  for a warning about the whole input.

### Warning codes

| Code | Meaning |
| --- | --- |
| `LOW_CONFIDENCE_SEGMENT` | A segment's beat fit or model confidence is low; the tempo of that span is uncertain. |
| `POSSIBLE_HALF_TIME` | The reading may be half the perceived tempo (a beat every two beats of the music). |
| `POSSIBLE_DOUBLE_TIME` | The reading may be double the perceived tempo. |
| `NO_DOWNBEATS` | No reliable downbeat was found; `bar` and `beatInBar` are `null`. |
| `SHORT_INPUT` | The input is shorter than a few bars, so the tempo is only a rough estimate. |
| `IRREGULAR_METER` | The bars do not fit one time signature. |
| `REFINE_SKIPPED` | The refinement did not run; `detail` gives the reason. |

## 3. Errors and exit codes

| Exit code | Meaning |
| --- | --- |
| 0 | Success |
| 2 | Usage error: unknown command or option, a missing or invalid value |
| 3 | A model file is missing (`MODEL_MISSING`) or its SHA-256 does not match (`MODEL_CHECKSUM`) |
| 4 | The input cannot be read or decoded (`INPUT_UNREADABLE`), or is too short to analyse (`INPUT_TOO_SHORT`) |
| 5 | Analysis failed for any other reason (`ANALYSIS_FAILED`) |

On any non-zero exit, stdout still carries one JSON object:

```json
{
  "schemaVersion": 1,
  "error": { "code": "MODEL_MISSING", "detail": "beat_this_final.onnx not found in models" }
}
```

`detail` is a human-readable sentence. It never repeats a value the user typed — not a path, a
file name, an unknown command or an option value — and it never contains a credential; the full
value is written to stderr instead. The one exception is the name of an unknown option when it
is a plain option name (`-x` or `--name`, ASCII letters, digits and `-`, at most 32 characters
after the dashes), as in `unknown option: --bogus`. A caller branches on `code`, not on the text.

## 4. Version JSON

```json
{
  "name": "opentune-beats",
  "version": "0.1.0",
  "beatThis": "1.1.0",
  "schemaVersion": 1,
  "models": {
    "final": { "file": "beat_this_final.onnx", "sha256": "…" },
    "small": { "file": "beat_this_small.onnx", "sha256": "…" },
    "mel": { "file": "mel_spectrogram.onnx", "sha256": "…" }
  }
}
```

`beatThis` is the version of the beat-this crate the binary was built with. `models` lists the
expected file name and SHA-256 of each model, which is what a caller compares a downloaded file
against.

## 5. Release assets

This section documents the contract that the release workflow will implement; nothing is
released yet.

| Asset | Contents |
| --- | --- |
| `opentune-beats-<version>-win-x64.zip` | `opentune-beats.exe`, `LICENSE`, `NOTICE`, `THIRD-PARTY-NOTICES` |
| `opentune-beats-<version>-macos-universal2.tar.gz` | `opentune-beats` (universal2), `LICENSE`, `NOTICE`, `THIRD-PARTY-NOTICES` |
| `mel_spectrogram.onnx`, `beat_this_final.onnx`, `beat_this_small.onnx` | Model files, platform independent, with the upstream SHA-256 |
| `SHA256SUMS` | The digest of every asset above |

The binaries carry the SHA-256 of each model they expect (section 4), so the model assets are
identified by the same digests. A caller installs a release into
`<data dir>/bin/opentune-beats-<version>-<target>/`, with the executable at its root and the
models in `models/` next to it; the binary's default `--models-dir` is exactly that directory,
so a release directory needs no arguments beyond the input file.

## 6. Current state

This build is the repository skeleton: the command surface, the exit codes and the error JSON
exist, `version` and `version --json` are complete, and `analyze` reports `ANALYSIS_FAILED`
with a detail saying that beat tracking is not linked yet. `beatThis` is `null` and `models` is
empty until the engine and the model pinning land; both fields are present so that a reader does
not have to special-case the skeleton. The analysis JSON of section 2 is what the engine will
produce, and is documented here so that the caller can be written against it now.
