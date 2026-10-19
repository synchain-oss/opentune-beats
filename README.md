# OpenTune Beats

Beat, downbeat and tempo-map analysis for backing tracks, as a small native command-line tool.

It is the beat-grid engine behind the `beat-grid` skill of
[OpenTune MCP Server](https://github.com/synchain-oss/opentune-mcp-server). An AI assistant
measures the tempo of a backing track section by section, then uses it to tighten the timing
of a vocal in OpenTune.

> **Status:** in development. There is no release yet.

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

Measured on accompaniments rendered from MIDI files with known tempo maps, including tempo
changes. Targets: median absolute beat error under 5 ms and 95th percentile under 15 ms. The
F-measure at ±70 ms is reported as well. The repository holds only the annotations and the
generation scripts, never audio.

## Platforms

CI builds Windows x64 and macOS (universal2) binaries.

## License

MIT; see [LICENSE](LICENSE). The Beat This! code and model weights are MIT licensed. Part of
the data the models were trained on is copyrighted or under restricted Creative Commons
licenses; see [NOTICE](NOTICE).

OpenTune Beats is made by Synchain. It is not an official OpenTune project.
