#!/usr/bin/env bash
# SPDX-FileCopyrightText: Copyright (c) 2026 Synchain
# SPDX-License-Identifier: MIT

# Inference smoke test of an unpacked release (.github/workflows/release.yml).
#
#   bash scripts/release-smoke.sh <install dir> <executable name> <scratch dir>
#
# <install dir> is laid out the way a caller installs a release (docs/output.md, section 5): the
# executable at its root and the three model files in `models/` next to it. The script renders a
# synthetic 120 BPM track of about ten seconds (bench/release-smoke.json, never committed as
# audio), runs `analyze` once with the `final` model and once with the `small` one, and checks
# the result. `--models-dir` is deliberately not passed: the binary has to find `models/` next to
# itself, which is the only thing a caller relies on.
#
# The checks: exit code 0, stdout is exactly one JSON object on one line, the envelope has the
# fields of the contract, and the dominant tempo is within 2 BPM of the 120 the track was
# rendered at. The binary verifies each model against the SHA-256 compiled into it before it
# loads one, so a wrong or damaged model file fails the run with exit code 3.
#
# Needs node (for the generator) and jq, which the GitHub runners have. Run from the repository
# root.

set -euo pipefail

if [ "$#" -ne 3 ]; then
  echo "usage: bash scripts/release-smoke.sh <install dir> <executable name> <scratch dir>" >&2
  exit 2
fi
install_dir=$1
exe=$2
work=$3

expected_bpm=120
tolerance_bpm=2

mkdir -p "$work"
wav="$work/smoke.wav"
node bench/gen.mjs --map bench/release-smoke.json --out "$wav"

status=0
for model in final small; do
  out="$work/smoke-$model.json"
  echo "::group::analyze --model $model"
  code=0
  "$install_dir/$exe" analyze "$wav" --model "$model" > "$out" || code=$?
  if [ "$code" != "0" ]; then
    echo "::error::analyze --model $model failed (exit code $code)"
    cat "$out" || true
    echo "::endgroup::"
    status=1
    continue
  fi
  lines=$(wc -l < "$out" | tr -d ' ')
  if [ "$lines" != "1" ]; then
    echo "::error::analyze --model $model wrote $lines lines to stdout, expected exactly one"
    echo "::endgroup::"
    status=1
    continue
  fi
  # -s reads the whole stream into an array, so a second document on stdout fails `length == 1`.
  if ! jq -e -s --arg model "$model" --argjson bpm "$expected_bpm" --argjson tolerance "$tolerance_bpm" '
      length == 1 and (.[0] |
        .schemaVersion == 1
        and .tool.name == "opentune-beats"
        and .model.name == $model
        and .input.sampleRateHz == 44100
        and ((.beats | length) >= 16)
        and (.summary.beats == (.beats | length))
        and ((.segments | length) >= 1)
        and (.warnings | type == "array")
        and (.summary.dominantBpm | type == "number")
        and (.summary.dominantBpm >= $bpm - $tolerance)
        and (.summary.dominantBpm <= $bpm + $tolerance))' "$out" > /dev/null; then
    echo "::error::analyze --model $model: the output does not match the contract or the tempo is not $expected_bpm +/- $tolerance_bpm BPM"
    cat "$out"
    echo "::endgroup::"
    status=1
    continue
  fi
  jq -r '"ok: model=\(.model.name) beats=\(.summary.beats) segments=\(.summary.segments) dominantBpm=\(.summary.dominantBpm)"' "$out"
  echo "::endgroup::"
done
exit "$status"
