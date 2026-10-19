#!/usr/bin/env node
// Score an `analyze` result against the tempo map it was rendered from.
//
// The thresholds are the acceptance criteria of the card: the median absolute error below 5 ms,
// the 95th percentile below 15 ms, and the standard +/-70 ms F-measure at least 0.95. The table
// is printed and, with --summary, appended to the GitHub step summary.

import fs from 'node:fs';

import { beatTimes } from './map.mjs';

const TOLERANCE_SEC = 0.07;
const MAX_MEDIAN_MS = 5;
const MAX_P95_MS = 15;
const MIN_F = 0.95;

function parseArgs(argv) {
  const options = { map: 'bench/tempo-map.json', analysis: null, summary: null };
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (arg === '--analysis' || arg === '--map' || arg === '--summary') {
      const value = argv[index + 1];
      if (value === undefined) throw new Error(`missing value for ${arg}`);
      options[arg.slice(2)] = value;
      index += 1;
    } else {
      throw new Error(`unknown option ${arg}`);
    }
  }
  if (!options.analysis) throw new Error('--analysis is required');
  return options;
}

function quantile(values, fraction) {
  if (values.length === 0) return 0;
  const sorted = [...values].sort((left, right) => left - right);
  const position = fraction * (sorted.length - 1);
  const low = Math.floor(position);
  const high = Math.ceil(position);
  if (low === high) return sorted[low];
  const weight = position - low;
  return sorted[low] * (1 - weight) + sorted[high] * weight;
}

function evaluate(analysis, map) {
  const truth = beatTimes(map).times;
  const estimates = analysis.beats.map((beat) => beat.timeSec).sort((left, right) => left - right);
  const used = new Array(estimates.length).fill(false);
  const errors = [];
  for (const time of truth) {
    let best = -1;
    let distance = Number.POSITIVE_INFINITY;
    for (let index = 0; index < estimates.length; index += 1) {
      if (used[index]) continue;
      const value = Math.abs(estimates[index] - time);
      if (value < distance && value <= TOLERANCE_SEC) {
        best = index;
        distance = value;
      }
    }
    if (best >= 0) {
      used[best] = true;
      errors.push(distance * 1000);
    }
  }
  const matched = errors.length;
  const precision = estimates.length === 0 ? 0 : matched / estimates.length;
  const recall = truth.length === 0 ? 0 : matched / truth.length;
  const f = precision + recall === 0 ? 0 : (2 * precision * recall) / (precision + recall);
  const median = quantile(errors, 0.5);
  const p95 = quantile(errors, 0.95);
  return {
    f,
    matched,
    truth: truth.length,
    estimates: estimates.length,
    median,
    p95,
    pass: f >= MIN_F && median <= MAX_MEDIAN_MS && p95 <= MAX_P95_MS,
  };
}

function table(result) {
  return [
    ['F-measure (70 ms)', result.f.toFixed(3), `>= ${MIN_F.toFixed(2)}`],
    ['matched beats', `${result.matched}/${result.truth}`, ''],
    ['extra beats', `${result.estimates - result.matched}`, ''],
    ['median |error|', `${result.median.toFixed(2)} ms`, `<= ${MAX_MEDIAN_MS} ms`],
    ['p95 |error|', `${result.p95.toFixed(2)} ms`, `<= ${MAX_P95_MS} ms`],
  ];
}

try {
  const options = parseArgs(process.argv.slice(2));
  const analysis = JSON.parse(fs.readFileSync(options.analysis, 'utf8'));
  const map = JSON.parse(fs.readFileSync(options.map, 'utf8'));
  const result = evaluate(analysis, map);
  const rows = table(result);
  const width = Math.max(...rows.map((row) => row[0].length));
  for (const row of rows) {
    console.log(`${row[0].padEnd(width)}  ${row[1].padStart(10)}  ${row[2]}`);
  }
  console.log(`verdict${' '.repeat(width - 7)}  ${result.pass ? 'PASS' : 'FAIL'}`);
  if (options.summary) {
    const markdown = [
      '### Accuracy on the synthetic tempo map',
      '',
      '| metric | value | threshold |',
      '| --- | --- | --- |',
      ...rows.map((row) => `| ${row[0]} | ${row[1]} | ${row[2]} |`),
      `| verdict | ${result.pass ? 'PASS' : 'FAIL'} | |`,
      '',
    ].join('\n');
    fs.appendFileSync(options.summary, markdown);
  }
  if (!result.pass) process.exitCode = 1;
} catch (error) {
  console.error(`score: ${error instanceof Error ? error.message : String(error)}`);
  process.exitCode = 1;
}
