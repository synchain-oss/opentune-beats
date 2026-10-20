#!/usr/bin/env node
// Render the synthetic backing track for the accuracy check.
//
// The track has the elements a backing track has, because a bare click track is not music to the
// beat model: an accented kick plus an open-hat/crash-like hit on beat 1 (the only accented beat,
// so the downbeat is identifiable), a snare on 2 and 4, a normal kick on 3, closed hats on every
// eighth note and a bass note per beat. A map may also ask for gaps (a sustained pad, no attacks)
// and for short pitched notes between the beats (vocal-like onsets that must not pull the
// millisecond refinement onto them).
//
// The noise comes from a fixed-seed generator: the same map always renders the same samples.
// The audio is never committed: the tempo map in bench/tempo-map.json or bench/maps/*.json is the
// truth, and this script can always recreate the audio from it.
//
// Usage: node bench/gen.mjs --map bench/maps/gaps.json --out bench/out/gaps.wav

import fs from 'node:fs';
import path from 'node:path';

import { beatGrid, gapSpans, isInGap } from './map.mjs';

const VOCAL_LENGTH_SEC = 0.16;
const VOCAL_ATTACK_SEC = 0.004;
const VOCAL_DECAY_SEC = 0.055;
const VOCAL_OFFSETS = [0.25, 0.4, 0.6, 0.75];
const VOCAL_FREQUENCIES = [440, 493.88, 523.25, 587.33];

function parseArgs(argv) {
  const options = { map: 'bench/tempo-map.json', out: null };
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (arg === '--map' || arg === '--out') {
      const value = argv[index + 1];
      if (value === undefined) throw new Error(`missing value for ${arg}`);
      options[arg.slice(2)] = value;
      index += 1;
    } else {
      throw new Error(`unknown option ${arg}`);
    }
  }
  if (!options.out) throw new Error('--out is required');
  return options;
}

function render(map) {
  const rate = map.sampleRate ?? 44_100;
  const grid = beatGrid(map);
  const beats = grid.beats;
  const gaps = gapSpans(map);
  const samples = new Float32Array(Math.ceil((grid.endSec + 1) * rate));
  let seed = 12_345;
  const noise = () => {
    seed = (Math.imul(seed, 1_664_525) + 1_013_904_223) >>> 0;
    return seed / 2 ** 31 - 1;
  };
  const add = (startSec, lengthSec, voice) => {
    const start = Math.round(startSec * rate);
    const length = Math.round(lengthSec * rate);
    for (let offset = 0; offset < length; offset += 1) {
      const position = start + offset;
      if (position >= samples.length) break;
      samples[position] += voice(offset / rate);
    }
  };
  // A sine whose pitch falls from 155 Hz to 45 Hz: the phase is the integral of the frequency.
  const kick = (at, gain) =>
    add(at, 0.25, (t) => {
      const phase = 2 * Math.PI * (45 * t + (110 * (1 - Math.exp(-40 * t))) / 40);
      return gain * Math.sin(phase) * Math.exp(-t / 0.09);
    });
  const snare = (at) =>
    add(at, 0.2, (t) => (0.45 * noise() + 0.3 * Math.sin(2 * Math.PI * 190 * t)) * Math.exp(-t / 0.05));
  // An open hat: longer than the closed one below, which is what makes beat 1 stand out.
  const crash = (at) => {
    let previous = 0;
    add(at, 0.4, (t) => {
      const value = noise();
      const highpassed = value - previous;
      previous = value;
      return 0.18 * highpassed * Math.exp(-t / 0.16);
    });
  };
  const hat = (at, gain) => {
    let previous = 0;
    add(at, 0.06, (t) => {
      const value = noise();
      const highpassed = value - previous;
      previous = value;
      return gain * highpassed * Math.exp(-t / 0.015);
    });
  };
  const bass = (at, frequency, lengthSec) =>
    add(at, lengthSec, (t) => 0.3 * Math.sin(2 * Math.PI * frequency * t) * Math.exp(-t / (lengthSec * 0.8)));
  // A slow-attack chord that fills a gap. It is the only sound there: no drum, hat, bass or vocal
  // attack is rendered inside a gap.
  const pad = (startSec, endSec) => {
    const duration = endSec - startSec;
    if (duration <= 0) return;
    const attack = Math.min(0.3, duration / 4);
    const release = Math.min(0.6, duration / 4);
    const chord = [130.81, 164.81, 196.0, 261.63]; // C major, one chord
    add(startSec, duration, (t) => {
      const envelope = Math.min(1, t / attack) * Math.min(1, (duration - t) / release);
      let value = 0;
      for (let index = 0; index < chord.length; index += 1) {
        value += Math.sin(2 * Math.PI * chord[index] * t + index * 0.7);
      }
      return (0.08 * envelope * value) / chord.length;
    });
  };
  const vocal = (at, frequency) =>
    add(at, VOCAL_LENGTH_SEC, (t) => {
      const envelope =
        Math.min(1, t / VOCAL_ATTACK_SEC) *
        Math.exp(-t / VOCAL_DECAY_SEC) *
        Math.min(1, (VOCAL_LENGTH_SEC - t) / 0.02);
      const partials =
        Math.sin(2 * Math.PI * frequency * t) +
        0.5 * Math.sin(4 * Math.PI * frequency * t) +
        0.3 * Math.sin(6 * Math.PI * frequency * t) +
        0.15 * Math.sin(8 * Math.PI * frequency * t);
      return (0.12 * envelope * partials) / 1.95;
    });
  const inGap = (timeSec) => isInGap(gaps, timeSec);
  // The last beat has no next beat to measure against. With a single-beat map, fall back to the
  // beat's own segment period instead of subtracting the missing previous beat (which was NaN).
  const periodAt = (index) => {
    if (index + 1 < beats.length) return beats[index + 1].timeSec - beats[index].timeSec;
    if (index > 0) return beats[index].timeSec - beats[index - 1].timeSec;
    return beats[index].periodSec;
  };

  for (let index = 0; index < beats.length; index += 1) {
    const beat = beats[index];
    const at = beat.timeSec;
    if (inGap(at)) continue;
    const period = periodAt(index);
    if (beat.beatInBar === 1) {
      kick(at, 1.0);
      crash(at);
    } else if (beat.beatInBar === 2 || beat.beatInBar === 4) {
      snare(at);
    } else {
      kick(at, 0.7);
    }
    hat(at, 0.22);
    hat(at + period / 2, 0.14);
    // Let the bass decay stop at the gap instead of ringing into the pad.
    let bassLength = period * 0.9;
    for (const gap of gaps) {
      if (gap.startSec > at && gap.startSec < at + bassLength) bassLength = gap.startSec - at;
    }
    if (bassLength > 0.01) bass(at, beat.beatInBar === 3 ? 98 : 73.4, bassLength);
  }

  for (const gap of gaps) pad(gap.startSec, gap.endSec);

  if (map.vocalOnsets === true) {
    for (let index = 0; index < beats.length; index += 1) {
      const beat = beats[index];
      if (inGap(beat.timeSec)) continue;
      const at = beat.timeSec + periodAt(index) * VOCAL_OFFSETS[index % VOCAL_OFFSETS.length];
      if (inGap(at)) continue;
      vocal(at, VOCAL_FREQUENCIES[index % VOCAL_FREQUENCIES.length]);
    }
  }

  return { samples, rate };
}

function encodeWav(samples, rate) {
  const bytes = Buffer.alloc(44 + samples.length * 2);
  bytes.write('RIFF', 0, 'ascii');
  bytes.writeUInt32LE(36 + samples.length * 2, 4);
  bytes.write('WAVE', 8, 'ascii');
  bytes.write('fmt ', 12, 'ascii');
  bytes.writeUInt32LE(16, 16);
  bytes.writeUInt16LE(1, 20);
  bytes.writeUInt16LE(1, 22);
  bytes.writeUInt32LE(rate, 24);
  bytes.writeUInt32LE(rate * 2, 28);
  bytes.writeUInt16LE(2, 32);
  bytes.writeUInt16LE(16, 34);
  bytes.write('data', 36, 'ascii');
  bytes.writeUInt32LE(samples.length * 2, 40);
  let peak = 0;
  for (const value of samples) peak = Math.max(peak, Math.abs(value));
  const gain = peak > 0.98 ? 0.98 / peak : 1;
  for (let index = 0; index < samples.length; index += 1) {
    const value = Math.max(-1, Math.min(1, samples[index] * gain));
    bytes.writeInt16LE(Math.round(value * 32_767), 44 + index * 2);
  }
  return bytes;
}

try {
  const options = parseArgs(process.argv.slice(2));
  const map = JSON.parse(fs.readFileSync(options.map, 'utf8'));
  const { samples, rate } = render(map);
  fs.mkdirSync(path.dirname(options.out), { recursive: true });
  fs.writeFileSync(options.out, encodeWav(samples, rate));
  console.error(`wrote ${options.out}: ${(samples.length / rate).toFixed(2)} s at ${rate} Hz`);
} catch (error) {
  console.error(`gen: ${error instanceof Error ? error.message : String(error)}`);
  process.exitCode = 1;
}
