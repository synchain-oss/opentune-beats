# OpenTune Beats

Beat, downbeat and tempo-map analysis for backing tracks, as a small native command-line tool.

It is the beat-grid engine behind the `beat-grid` skill of
[OpenTune MCP Server](https://github.com/synchain-oss/opentune-mcp-server). An AI assistant
measures the tempo of a backing track section by section, then uses it to tighten the timing
of a vocal in OpenTune.

> **Status:** early release (0.1.0). The binaries and the model files are on the
> [Releases](https://github.com/synchain-oss/opentune-beats/releases) page.

[中文说明](README.zh-CN.md)

## What it does

- **Input:** a WAV file of the backing track or the full mix. Not an isolated vocal: beat
  trackers are unreliable on a vocal alone. If you only have a dry vocal, give the tempo
  yourself (a BPM, a click track or a MIDI file).
- **Output:** JSON on stdout:
  - every beat and downbeat time, with a confidence value;
  - a tempo map: constant-tempo segments with their start, end, BPM, time-signature
    candidates and confidence, with half-time and double-time readings reconciled;
  - beat times refined to the millisecond against the onset envelope of the audio;
  - the tool version, the model name and its SHA-256, and the processing parameters.
- **Models:** `--model final` (the default, about 80 MB, best accuracy) or `--model small`
  (about 10 MB).

OpenTune projects have a single tempo. The per-section tempo map is for correcting vocal
timing; setting the project tempo stays a separate choice for the user.

## How it works

Beat and downbeat activations come from
[Beat This!](https://github.com/CPJKU/beat_this) (CPJKU, ISMIR 2024) through
[beat-this-rs](https://github.com/danigb/beat-this-rs), a pure-Rust port: no Python, no
PyTorch and no system libraries. The tempo segmentation, the half-time and double-time
reconciliation and the millisecond refinement are our own code, written from published
descriptions.

## Accuracy

Measured on accompaniments rendered from MIDI or generated from parameters with known tempo
maps, including tempo changes, breakdowns and off-beat vocal-like onsets. Targets: median
absolute beat error under 5 ms and 95th percentile under 15 ms; the F-measure at ±70 ms is
reported as well. The same synthetic material gates the tempo map itself: one segment for a
steady track, a segment count within one of the truth for a multi-tempo track, every segment's
BPM within 1 % of the truth, a confidence of at least 0.8 on steady material, and a refinement
that does not widen the inter-beat interval spread. The beat F-measure of the multi-tempo
material is reported but not gated, because the model carries the old tempo for a while after a
change (see the limitations in [docs/output.md](docs/output.md)). The repository holds only the
annotations and the generation scripts, never audio.

## Limitations

- A tempo change is followed with a delay: the model keeps the previous tempo for a while, so the
  beats around a change can be a frame or more late.
- A breakdown is not invented: `beats` keeps only the beats the model reported. A gap between two
  segments is described by `segments[].gapAfterSec`, and a caller that needs the beat times inside
  it derives them from `bpm`.
- The refinement is only applied when it clearly improves a segment; when it would not, the
  segment keeps the model's times and reports `refinement: "skipped"`.
- The confidence is a fit quality, not a probability that the tempo is right.
- Only WAV is promised; inputs are limited to 2 GiB, one hour and a sample rate of 8 000 to
  384 000 Hz, and RF64/BW64 is refused.

## Platforms

CI builds Windows x64 and macOS (universal2) binaries. Release assets — the two archives, the
three model files and `SHA256SUMS` — are on the
[Releases](https://github.com/synchain-oss/opentune-beats/releases) page; see
[docs/RELEASE.md](docs/RELEASE.md) for how a release is built.

## License

MIT; see [LICENSE](LICENSE). The Beat This! code and model weights are MIT licensed. Part of
the data the models were trained on is copyrighted or under restricted Creative Commons
licenses; see [NOTICE](NOTICE).

OpenTune Beats is made by Synchain. It is not an official OpenTune project.
