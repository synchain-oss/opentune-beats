#!/usr/bin/env node
// SPDX-FileCopyrightText: Copyright (c) 2026 Synchain
// SPDX-License-Identifier: MIT

// Regression tests for the synthetic generator (bench/gen.mjs). The audio is rendered into a
// scratch directory and read back; nothing is written into the repository. Run with:
//
//   node --test bench/gen.test.mjs

import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const GEN = fileURLToPath(new URL('./gen.mjs', import.meta.url));
const RATE = 22_050;
const HEADER_BYTES = 44;

/** Renders a map into a scratch directory and returns the 16-bit samples as floats in -1..1. */
function render(t, map) {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'ot-gen-'));
  t.after(() => fs.rmSync(directory, { recursive: true, force: true }));
  const mapFile = path.join(directory, 'map.json');
  const wavFile = path.join(directory, 'out.wav');
  fs.writeFileSync(mapFile, JSON.stringify(map));
  const run = spawnSync(process.execPath, [GEN, '--map', mapFile, '--out', wavFile], {
    encoding: 'utf8',
  });
  assert.equal(run.status, 0, run.stderr);
  const bytes = fs.readFileSync(wavFile);
  assert.equal(bytes.toString('ascii', 0, 4), 'RIFF');
  assert.equal(bytes.readUInt32LE(24), RATE);
  const samples = new Float64Array((bytes.length - HEADER_BYTES) / 2);
  for (let index = 0; index < samples.length; index += 1) {
    samples[index] = bytes.readInt16LE(HEADER_BYTES + index * 2) / 32_768;
  }
  return samples;
}

/** The RMS of the samples between two times, in seconds. */
function rms(samples, fromSec, toSec) {
  const from = Math.round(fromSec * RATE);
  const to = Math.round(toSec * RATE);
  let sum = 0;
  for (let index = from; index < to; index += 1) sum += samples[index] * samples[index];
  return Math.sqrt(sum / (to - from));
}

test('a map with a single beat renders the whole beat, with a finite period', (t) => {
  // One bar of one beat at 120 BPM, the beat at 0.5 s. The last (here: only) beat has no next
  // beat to measure a period against; it must fall back to its own segment period, not NaN.
  const samples = render(t, {
    schema: 1,
    sampleRate: RATE,
    beatsPerBar: 1,
    leadInSec: 0.5,
    segments: [{ bpm: 120, bars: 1 }],
  });
  // The kick has ended (0.25 s) and so have the crash (0.4 s) and the hats; between 0.41 and
  // 0.45 s after the beat only the bass note (period * 0.9 = 0.45 s long) can still sound. With a
  // NaN period the bass and the half-period hat are dropped and this window is silent.
  const tail = rms(samples, 0.5 + 0.41, 0.5 + 0.45);
  assert.ok(tail > 0.02, `the bass note is missing: RMS ${tail}`);
  assert.ok(rms(samples, 0.5, 0.5 + 0.05) > 0.1, 'the attack is missing');
});

test('the same map always renders the same samples', (t) => {
  const map = {
    schema: 1,
    sampleRate: RATE,
    beatsPerBar: 2,
    leadInSec: 0.25,
    segments: [{ bpm: 100, bars: 2 }],
  };
  assert.deepEqual(render(t, map), render(t, map));
});
