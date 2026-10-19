#!/usr/bin/env node
// Render the synthetic backing track for the accuracy check.
//
// A plain drum loop: kick on beats 1 and 3, snare on 2 and 4, closed hats on every eighth note
// and a bass note per beat. A bare click track is not music to the beat model and makes it lose
// the tempo after a change, so the track is built from the elements a backing track has. The
// noise comes from a fixed-seed generator: the same map always renders the same samples. The
// audio is never committed: the tempo map in bench/tempo-map.json is the truth, and this script
// can always recreate the audio from it.

import fs from 'node:fs';
import path from 'node:path';

import { beatTimes } from './map.mjs';

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
  const beatsPerBar = map.beatsPerBar ?? 4;
  const { times, endSec } = beatTimes(map);
  const samples = new Float32Array(Math.ceil((endSec + 1) * rate));
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
  const kick = (at) =>
    add(at, 0.25, (t) => {
      const phase = 2 * Math.PI * (45 * t + (110 * (1 - Math.exp(-40 * t))) / 40);
      return 0.7 * Math.sin(phase) * Math.exp(-t / 0.09);
    });
  const snare = (at) =>
    add(at, 0.2, (t) => (0.45 * noise() + 0.3 * Math.sin(2 * Math.PI * 190 * t)) * Math.exp(-t / 0.05));
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
  for (let index = 0; index < times.length; index += 1) {
    const at = times[index];
    const period = index + 1 < times.length ? times[index + 1] - at : at - times[index - 1];
    const inBar = index % beatsPerBar;
    if (inBar % 2 === 0) kick(at);
    else snare(at);
    hat(at, 0.22);
    hat(at + period / 2, 0.14);
    bass(at, inBar === 2 ? 98 : 73.4, period * 0.9);
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
