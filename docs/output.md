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
| `version` | One line: `opentune-beats <version> (beat-this <crate version>)` |
| `version --json` | The version JSON of section 4 |
| `--version` | The same one line as `version` |
| `--help`, `-h`, `help` | The usage text on stderr; stdout stays empty; exit code 0 |

`analyze` options:

| Option | Default | Meaning |
| --- | --- | --- |
| `<input>` | – | A WAV file (PCM 8/16/24/32-bit or float32, mono or stereo; up to 8 channels are averaged to mono). The sample rate must be from 8 000 to 384 000 Hz and the audio at most 3 600 seconds (one hour) long; a file outside either limit is `INPUT_UNREADABLE`. 8-bit PCM is the unsigned form WAV uses. Other formats that the decoder handles are supported on a best-effort basis; only WAV is promised. The file may be at most 2 GiB; RF64/BW64 files are refused with a clear message, as is a `data` chunk larger than the file. |
| `--model final\|small` | `final` | Which beat model to run. `final` is about 80 MB and more accurate; `small` is about 10 MB. |
| `--models-dir <dir>` | `<directory of the executable>/models` | Where the model files are read from. |
| `--pretty` | off | Indent the JSON output. |
| `--no-refine` | off | Skip the millisecond refinement against the onset envelope. |
| `--min-segment-beats <n>` | `8` | A constant-tempo segment shorter than this many beats is merged into its neighbour. |

`--models-dir` must contain `mel_spectrogram.onnx` and, depending on `--model`,
`beat_this_final.onnx` or `beat_this_small.onnx`. The SHA-256 of each file is compiled into the
binary and checked before the model is loaded; a mismatch is exit code 3 with `MODEL_CHECKSUM`,
a missing file is exit code 3 with `MODEL_MISSING`. A path that exists but cannot be read (no
permission, a directory where the file should be) is also exit code 3 with `MODEL_MISSING`: there
is no separate code for it, but its `detail` reads "exists but cannot be read" with the reason
instead of "is missing". Nothing is downloaded at run time.

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
      "octaveNote": null, "refinement": "applied", "gapAfterSec": null }
  ],
  "tempoChanges": [ { "atSec": 0.0, "fromBpm": 0.0, "toBpm": 0.0, "kind": "step" } ],
  "refinement": { "applied": true, "globalOffsetMs": 0.0, "medianAdjustMs": 0.0, "p95AdjustMs": 0.0,
                  "segmentsApplied": 0, "segmentsSkipped": 0,
                  "ioiSpreadBeforeMs": 0.0, "ioiSpreadAfterMs": 0.0 },
  "summary": { "beats": 0, "downbeats": 0, "segments": 0, "dominantBpm": 0.0, "lowConfidenceSegments": [] },
  "warnings": [ { "code": "LOW_CONFIDENCE_SEGMENT", "detail": "…", "segment": 0 } ]
}
```

- `input.fileName` is the file name only, never the directory: the output must not leak where the
  file lives on the caller's machine. `input.durationSec`, `sampleRateHz` and `channels` describe
  the decoded audio.
- `params` echoes the parameters the run actually used, so a stored result can be reproduced.
  `maxRefineMs` is the window the refinement may move a beat; `frameSec` is the hop of the model's
  frame grid.
- `beats[].timeSec` is the final time, `beats[].rawTimeSec` the model's time before refinement.
  `confidence` is 0..1. `index` counts from 0 in time order. `segment` is the index into
  `segments`. A beat inserted by the half-time/double-time reconciliation (below) has an
  interpolated `rawTimeSec`: it is the mean of the two model beats it lies between.
- `bar` and `beatInBar` count from 1 and are `null` when there is no reliable downbeat; a
  `NO_DOWNBEATS` warning says so. `isDownbeat` is then `false` for every beat.
- `segments` are the constant-tempo spans: they cover the whole input, are contiguous and do not
  overlap. `startSec` and `endSec` are the span, `firstBeat` and `lastBeat` the beat indices in it.
  `bpm` is the tempo and `beatPeriodSec` its reciprocal in seconds (six decimals, so the two stay
  consistent). `timeSignature` is the best
  reading and `timeSignatureCandidates` the scored alternatives. `fitResidualMs` describes how
  well a constant grid fits the beats in the span.
- `segments[].octaveNote` is `"halved"`, `"doubled"` or `null`: set when a segment's reading was
  reconciled with the neighbouring ones because the tempo was a factor of two off.
- `segments[].refinement` is `"applied"` when the millisecond refinement was adopted for that
  segment and `"skipped"` when the model's raw times were kept: the refinement would not have made
  the segment clearly better, or it did not run at all (`--no-refine`, no usable audio). The value
  is decided on the final segments, after the segments have been detected again on the refined
  beats: a segment is `"applied"` when at least one beat inside it carries a refined time.
  `refinement.applied` at the top level is true when at least one segment is `"applied"`.
- `segments[].gapAfterSec` is the time between this segment's last beat and the next segment's
  first beat when the model reported no beat in between (a breakdown), and `null` when the two are
  contiguous. The missing beats are not inserted into `beats`: a caller that needs them derives
  them from `bpm`. A gap short enough to stay inside one segment is not reported here; the fit
  still counts its missing beats, so the tempo is not bent by it.
- `tempoChanges` lists the boundaries between segments: `atSec` is the boundary, `fromBpm` and
  `toBpm` the tempos on either side, `kind` is `"step"` for an abrupt change and `"ramp"` for a
  gradual one. `step` means one interval-to-interval jump carries most of the change; `ramp` means
  the intervals drift across the boundary (or the change is too small to be called a step).
- `refinement.applied` is `false` when `--no-refine` was given, when the refinement could not run
  (a `REFINE_SKIPPED` warning then explains why), or when no segment cleared its acceptance check.
  `globalOffsetMs` is the beat-count-weighted median of the common offsets of the segments whose
  refinement was adopted (0 when none). With one segment it is that segment's offset. With several
  segments of which only some were adopted, the segments that kept the model's times do not count,
  so it is a summary of the adopted segments and not a shift that every beat received.
  `medianAdjustMs` and `p95AdjustMs` describe the per-beat adjustments that were applied *beyond
  the common offset of the beat's own segment* (not beyond `globalOffsetMs`). `segmentsApplied`
  and `segmentsSkipped` count the final segments of `segments` by their `refinement` value. `ioiSpreadBeforeMs` and `ioiSpreadAfterMs` are the
  p95-p5 spread of the one-beat intervals before and after the refinement; the refinement is
  designed so the second never exceeds the first by more than its own ~1 ms resolution (2 ms).
- `summary` is the aggregate a caller can show without walking the arrays.
  `lowConfidenceSegments` lists the indices of the segments whose confidence is below the
  threshold (0.5), in ascending order.
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
| 2 | Usage error (`USAGE`): unknown command or option, a missing or invalid value |
| 3 | A model file is missing or cannot be read (`MODEL_MISSING`) or its SHA-256 does not match (`MODEL_CHECKSUM`) |
| 4 | The input cannot be read or decoded (`INPUT_UNREADABLE`), including a sample rate outside 8 000 to 384 000 Hz and audio longer than 3 600 seconds, or is too short to analyse (`INPUT_TOO_SHORT`) |
| 5 | Analysis failed for any other reason (`ANALYSIS_FAILED`), including an internal error (a panic) in the decoder, the post-processing or a dependency |

If stdout cannot be written (for example the reader closed the pipe) after an `analyze` run that
would have exited 0, the run exits 5 instead. This is a behaviour change for analysis runs: the
analysis result was lost, and a caller that reads stdout must be able to tell. A run that already
exits non-zero keeps its code (2, 3, 4, 5), so the error class is not hidden. `version`,
`--version` and `--help` still exit 0 when stdout cannot be written, as before.

A panic inside the analysis is caught: the run exits 5 with `ANALYSIS_FAILED` and a fixed `detail`
("the analysis stopped on an internal error"), and one line with the panic message goes to stderr.
A panic therefore never ends the process without the error JSON.

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

This section documents the contract that the release workflow
([`.github/workflows/release.yml`](../.github/workflows/release.yml), described in
[RELEASE.md](./RELEASE.md)) implements; no release has been published yet.

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

## 6. How the grid is produced

The pipeline is fixed, and a stored result can be reproduced from `params` and `model`:

1. **Input.** The file is read and decoded as WAV (PCM 8/16/24/32 or IEEE float 32, a sample rate
   of 8 000 to 384 000 Hz, any channel count up to 8, averaged to mono). A sample rate outside that
   range is refused, and so is audio longer than 3 600 seconds (one hour): the duration is the
   number of frames in the `data` chunk divided by the sample rate, and it is checked before any
   sample is decoded or resampled. Both are `INPUT_UNREADABLE` (exit code 4), with a `detail` that
   names the limit. A file larger than 2 GiB is refused, and at
   most 2 GiB and one byte of it is ever read; a `data` chunk whose declared size is `0xFFFFFFFF` (a streaming writer) is read to
   the end of the file; RF64/BW64, which needs a 64-bit size field, is refused with a message that
   says so. A file that is not a readable WAV is
   `INPUT_UNREADABLE`; one shorter than half a second is `INPUT_TOO_SHORT`.
2. **Models.** `mel_spectrogram.onnx` and the beat model of `--model` are hashed before they are
   loaded. The expected digests are compilation constants (`version --json` publishes them), so a
   swapped or corrupted file is `MODEL_CHECKSUM`, and a missing one is `MODEL_MISSING` (so is one
   that exists but cannot be read, with a `detail` that says so).
3. **Inference.** beat-this (the `beat-this` crate, pure-Rust `rten` runtime) resamples to
   22 050 Hz, computes the log-mel spectrogram and runs the transformer over 30 s chunks. It
   returns beat times and downbeat times on a 20 ms frame grid; each beat keeps that frame's
   beat probability as its `confidence`.
4. **Refinement** (skipped with `--no-refine`). A 1 ms energy envelope of the decoded audio is
   compared around every beat. Each segment gets a **common offset** first: the median of the
   per-beat snap shifts, bounded by `maxRefineMs`, which moves the whole segment onto the attacks
   without touching its interval spread. Then a beat moves further, onto its own attack, only when
   that brings it closer to the local tempo line through its neighbouring beats. A segment adopts
   its refined times only when they are clearly better (the fit-residual p95, the interval spread
   or the distance to the attacks improves by at least 2 ms) and the interval spread does not grow
   by more than 2 ms (twice the envelope's ~1 ms resolution); otherwise the segment keeps the
   model's times and reports `refinement: "skipped"`. The beats are then put back in time order, clamped to
   `[0, durationSec]`, and a beat within 1 ms of the one before it is dropped. The numbers are
   reported in `refinement`; `--no-refine`, or an input whose audio has no attack to refine
   against, leaves `applied` false and adds `REFINE_SKIPPED`. An input with nothing for the model to
   follow (digital silence, a constant tone) does not get that far: the model reports fewer than two
   beats and the run fails with `ANALYSIS_FAILED` (exit code 5, no analysis JSON).
5. **Tempo map.** The interval between two beats is divided by the number of beats it spans
   (`round(gap / period)`, against the median of the intervals on the side that interval agrees
   with), so a breakdown where the model reported no beat is not read as one very long beat and a
   sudden tempo drop is not read as a gap. The beats are split where the local
   per-beat interval changes by more than 4 % over four beats; a span shorter than
   `minSegmentBeats` is merged into its neighbour, and neighbouring spans whose tempos differ by
   less than 1.5 % are merged when one straight line still fits them (p95 residual under 60 ms).
   Each span is fitted by least squares to `time = t0 + k * period` on those grid indices, which
   gives `bpm`, `beatPeriodSec` and `fitResidualMs`. A gap longer than eight missing beats ends the
   span, so the tempo after a breakdown may differ; the two sides are merged again when they agree,
   which is why a steady song with a breakdown stays one segment. Spans are contiguous and cover
   the whole input, and the boundaries become `tempoChanges`.
6. **Octave reconciliation.** A span whose period is a factor of two off the other spans is
   reconciled with them: a double-time span keeps every second beat (`octaveNote: "halved"`) and a
   half-time span gains the midpoints (`octaveNote: "doubled"`), including the one that fills the
   interval to the next segment. `POSSIBLE_HALF_TIME` and `POSSIBLE_DOUBLE_TIME` report a whole
   reading that may be an octave away from the perceived tempo without changing it.
7. **Bars.** Downbeats are snapped to the nearest beat (within 60 ms). The most common distance
   between downbeats is the time signature and its share of the downbeats is the candidate score;
   an isolated downbeat that splits one bar of that meter in two is dropped as an extra mark.
   Beats are numbered from the downbeats, and a bar also starts every `meter` beats when the model
   missed one. Beats before an unreliable first downbeat keep `bar` and `beatInBar` as `null`. A
   grid without two downbeats gets `NO_DOWNBEATS`; one whose remaining downbeat distances still
   disagree with the meter gets `IRREGULAR_METER` (at least two misaligned gaps, and at least a
   quarter of them, so one stray downbeat is not a meter change).
8. **Warnings and summary.** `SHORT_INPUT` marks an input under a few bars, and
   `LOW_CONFIDENCE_SEGMENT` a span whose confidence is under 0.5. A segment's confidence is the
   weighted mean of three 0..1 readings: the model's mean beat probability (lifted from 0.7, so a
   mediocre activation alone cannot condemn a well-fitted segment), the fit-residual p95 scaled by
   160 ms, and the share of beats inside 80 ms of the fitted line. `summary` repeats the counts a
   caller needs without walking the arrays.

### Known limitations

- **A tempo change is followed with a delay.** The model keeps the previous tempo for a while
  after the music changes, so the first segment after a change can be short or slightly off; the
  beat times around the change can be a frame or more late. The accuracy bench reports the beat
  F-measure of the multi-tempo material but does not gate it for that reason; the tempo-map gate
  still checks every reported segment's BPM.
- **A breakdown is not interpolated in `beats`.** The grid keeps only the beats the model
  reported; a gap is described by `segments[].gapAfterSec`, and a caller that needs the beat times
  inside it derives them from `bpm`.
- **A refinement that would not help is not applied.** If the model's grid already fits its own
  tempo line, the refinement reports `refinement: "skipped"` for that segment and keeps the
  raw times, even when the attacks sit a few milliseconds away. This is deliberate: the check
  exists so a refinement can never add jitter, and it never widens the interval spread by more
  than the ~1 ms resolution of the onset envelope.
- **`ramp` is a small or gradual change.** `kind` is `"step"` when one interval-to-interval jump
  carries most of the change across a boundary, and `"ramp"` otherwise; a drift too small to be
  heard as a tempo change is therefore reported as `"ramp"`.
- **The confidence is a fit quality, not a probability.** It describes how well one constant-tempo
  line explains the span; it is not the probability that the tempo is right.

`error.detail` never repeats a value the user typed: no path, file name, unknown command or option
value. The full value goes to stderr, which is where a caller looks when a run fails.
