#!/usr/bin/env node
// Score an `analyze` result against the tempo map it was rendered from.
//
// The beat gate is unchanged: the median absolute error below 5 ms, the 95th percentile below
// 15 ms, and the standard +/-70 ms F-measure at least 0.95. Truth beats inside a gap are excluded
// because the material has no percussion there.
//
// A map whose acceptance.tempoMap is true also gets the tempo-map gates:
//   - the segment count matches the truth (exactly one for steady material, truth +/- 1 for a
//     multi-tempo map);
//   - every reported segment's BPM is within 1 % of the truth at its midpoint, and every truth
//     segment is represented by a reported segment (its midpoint or its largest overlap);
//   - every reported segment of a steady map has confidence >= 0.8;
//   - the p95-p5 spread of consecutive refined beat intervals is no larger than the same spread
//     of the model's raw intervals, up to the onset envelope's own ~1 ms resolution (2 ms).
//
// The tables are printed; with --summary they are also appended to the GitHub step summary.

import fs from 'node:fs';
import path from 'node:path';

import { beatTimes, gapSpans, isInGap, segmentSpans } from './map.mjs';

const TOLERANCE_SEC = 0.07;
const MAX_MEDIAN_MS = 5;
const MAX_P95_MS = 15;
const MIN_F = 0.95;

const MAX_BPM_ERROR_PCT = 1;
const MIN_STEADY_CONFIDENCE = 0.8;
// The onset envelope is sampled every ~1 ms (the hop is rounded to whole samples), so a refined
// grid can differ from a machine-steady raw grid by that much even when the refinement is right.
// The gate allows that resolution and nothing more; the failure it exists for is a refinement
// that widens the spread by tens of milliseconds.
const DISPERSION_SLACK_SEC = 0.002;

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

function sortedTimes(beats, key) {
  return beats
    .map((beat) => Number(beat[key]))
    .filter((value) => Number.isFinite(value))
    .sort((left, right) => left - right);
}

// p95 - p5 of the consecutive inter-beat intervals, in seconds. An interval whose middle falls in
// a gap is skipped: it spans a silence where the material has no beat, and a single such interval
// would otherwise dominate the number. Null when fewer than two intervals remain, so a missing
// signal cannot silently pass the refinement gate.
function interBeatSpreadSec(times, gaps = []) {
  if (times.length < 2) return null;
  const intervals = [];
  for (let index = 1; index < times.length; index += 1) {
    const start = times[index - 1];
    const end = times[index];
    const middle = (start + end) / 2;
    if (gaps.some((span) => middle >= span.startSec && middle < span.endSec)) continue;
    intervals.push(end - start);
  }
  if (intervals.length < 2) return null;
  return quantile(intervals, 0.95) - quantile(intervals, 0.05);
}

function overlapSec(leftStart, leftEnd, rightStart, rightEnd) {
  if (![leftStart, leftEnd, rightStart, rightEnd].every(Number.isFinite)) return 0;
  return Math.max(0, Math.min(leftEnd, rightEnd) - Math.max(leftStart, rightStart));
}

function evaluateBeats(analysis, map) {
  const gaps = gapSpans(map);
  const truth = beatTimes(map).times.filter((time) => !isInGap(gaps, time));
  const beats = Array.isArray(analysis.beats) ? analysis.beats : [];
  // Symmetric with the truth: inside a gap there is no percussion, so an estimate there is
  // neither a match nor a false positive. The tempo map gate is what judges a breakdown.
  const estimates = sortedTimes(beats, 'timeSec').filter((time) => !isInGap(gaps, time));
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

function midpointOf(segment) {
  if (!Number.isFinite(segment.startSec) || !Number.isFinite(segment.endSec)) return null;
  return (segment.startSec + segment.endSec) / 2;
}

function truthIndexAt(truth, timeSec) {
  if (timeSec === null || truth.length === 0) return -1;
  for (let index = 0; index < truth.length; index += 1) {
    if (timeSec >= truth[index].startSec && timeSec < truth[index].endSec) return index;
  }
  // The reported segments cover the whole input, the truth spans only start at the first beat and
  // end at the last one. A midpoint in the lead-in or in the trailing silence belongs to the
  // nearest edge span, so it is compared with that tempo instead of failing outright.
  return timeSec < truth[0].startSec ? 0 : truth.length - 1;
}

function evaluateTempoMap(analysis, map) {
  const truth = segmentSpans(map);
  const rawSegments = Array.isArray(analysis.segments) ? analysis.segments : [];
  const reported = rawSegments
    .map((segment) => ({
      startSec: Number(segment.startSec),
      endSec: Number(segment.endSec),
      bpm: Number(segment.bpm),
      confidence: Number(segment.confidence),
      midpoint: null,
      truthIndex: -1,
      bpmErrorPct: null,
    }))
    .sort((left, right) => {
      if (!Number.isFinite(left.startSec)) return 1;
      if (!Number.isFinite(right.startSec)) return -1;
      return left.startSec - right.startSec;
    });
  for (const segment of reported) {
    segment.midpoint = midpointOf(segment);
    segment.truthIndex = truthIndexAt(truth, segment.midpoint);
    if (segment.truthIndex >= 0 && Number.isFinite(segment.bpm)) {
      const trueBpm = truth[segment.truthIndex].bpm;
      segment.bpmErrorPct = (Math.abs(segment.bpm - trueBpm) / trueBpm) * 100;
    }
  }

  const steady = map.segments.length === 1;
  const countPass = steady ? reported.length === 1 : Math.abs(reported.length - truth.length) <= 1;

  const maxBpmErrorPct =
    reported.length > 0 && reported.every((segment) => segment.bpmErrorPct !== null)
      ? Math.max(...reported.map((segment) => segment.bpmErrorPct))
      : null;
  const bpmPass = maxBpmErrorPct !== null && maxBpmErrorPct <= MAX_BPM_ERROR_PCT + 1e-9;

  // A reported segment represents the truth segment it has the largest overlap with. A segment
  // whose midpoint lies in a truth span also represents it. Requiring both checks to cover every
  // truth span catches a tempo that is missing entirely, even when the count gate allows +/-1.
  const largestOverlap = reported.map((segment) => {
    let best = -1;
    let bestValue = 0;
    for (let index = 0; index < truth.length; index += 1) {
      const value = overlapSec(segment.startSec, segment.endSec, truth[index].startSec, truth[index].endSec);
      if (value > bestValue) {
        best = index;
        bestValue = value;
      }
    }
    return best;
  });
  const represented = truth.map((span, truthIndex) =>
    reported.some((segment, reportedIndex) => {
      if (segment.midpoint !== null && segment.midpoint >= span.startSec && segment.midpoint < span.endSec) {
        return true;
      }
      return largestOverlap[reportedIndex] === truthIndex;
    }),
  );
  const covered = represented.filter(Boolean).length;
  const coveragePass = represented.every(Boolean);

  const confidences = reported.map((segment) => segment.confidence);
  const minConfidence =
    confidences.length > 0 && confidences.every(Number.isFinite) ? Math.min(...confidences) : null;
  const confidencePass = !steady || (minConfidence !== null && minConfidence >= MIN_STEADY_CONFIDENCE);

  const beats = Array.isArray(analysis.beats) ? analysis.beats : [];
  const gaps = gapSpans(map);
  const refinedSpreadSec = interBeatSpreadSec(sortedTimes(beats, 'timeSec'), gaps);
  const rawSpreadSec = interBeatSpreadSec(sortedTimes(beats, 'rawTimeSec'), gaps);
  const dispersionPass =
    refinedSpreadSec !== null &&
    rawSpreadSec !== null &&
    refinedSpreadSec <= rawSpreadSec + DISPERSION_SLACK_SEC;

  return {
    steady,
    truth,
    reported,
    truthCount: truth.length,
    covered,
    countPass,
    maxBpmErrorPct,
    bpmPass,
    coveragePass,
    confidencePass,
    minConfidence,
    rawSpreadSec,
    refinedSpreadSec,
    dispersionPass,
    pass: countPass && bpmPass && coveragePass && confidencePass && dispersionPass,
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

function printTable(rows) {
  const width = Math.max(...rows.map((row) => row[0].length));
  for (const row of rows) {
    console.log(`${row[0].padEnd(width)}  ${row[1].padStart(10)}  ${row[2]}`);
  }
}

function formatNumber(value, digits = 2) {
  return Number.isFinite(value) ? value.toFixed(digits) : 'n/a';
}

function formatMs(seconds) {
  return seconds === null || !Number.isFinite(seconds) ? 'n/a' : `${(seconds * 1000).toFixed(2)} ms`;
}

function verdict(pass) {
  return pass ? 'PASS' : 'FAIL';
}

function tempoRows(result) {
  const countThreshold = result.steady ? 'exactly 1' : `truth ${result.truthCount} +/- 1`;
  const rows = [
    ['reported segments', String(result.reported.length), countThreshold, verdict(result.countPass)],
    [
      'max segment BPM error',
      result.maxBpmErrorPct === null ? 'n/a' : `${result.maxBpmErrorPct.toFixed(2)} %`,
      `<= ${MAX_BPM_ERROR_PCT} %`,
      verdict(result.bpmPass),
    ],
    [
      'truth segments covered',
      `${result.covered}/${result.truthCount}`,
      'all',
      verdict(result.coveragePass),
    ],
    [
      'min segment confidence',
      formatNumber(result.minConfidence, 2),
      result.steady ? `>= ${MIN_STEADY_CONFIDENCE.toFixed(2)}` : 'steady material only',
      result.steady ? verdict(result.confidencePass) : '-',
    ],
    ['IOI spread raw', formatMs(result.rawSpreadSec), '', ''],
    [
      'IOI spread refined',
      formatMs(result.refinedSpreadSec),
      `<= raw (${formatMs(result.rawSpreadSec)})`,
      verdict(result.dispersionPass),
    ],
  ];
  return rows;
}

function printTempoTable(rows) {
  const header = ['metric', 'value', 'threshold', 'status'];
  const widths = header.map((cell, index) =>
    Math.max(cell.length, ...rows.map((row) => (row[index] ?? '').length)),
  );
  console.log(
    `${header[0].padEnd(widths[0])}  ${header[1].padStart(widths[1])}  ${header[2].padEnd(widths[2])}  ${header[3]}`,
  );
  for (const row of rows) {
    console.log(
      `${row[0].padEnd(widths[0])}  ${row[1].padStart(widths[1])}  ${(row[2] ?? '').padEnd(widths[2])}  ${row[3] ?? ''}`,
    );
  }
}

function printSegmentTable(reported, truth) {
  if (reported.length === 0) {
    console.log('reported segments: none');
    return;
  }
  const header = ['#', 'span (s)', 'bpm', 'truth bpm', 'error', 'confidence'];
  const rows = reported.map((segment, index) => {
    const span =
      Number.isFinite(segment.startSec) && Number.isFinite(segment.endSec)
        ? `${segment.startSec.toFixed(2)}-${segment.endSec.toFixed(2)}`
        : 'n/a';
    return [
      String(index),
      span,
      formatNumber(segment.bpm, 2),
      segment.truthIndex >= 0 ? formatNumber(truth[segment.truthIndex].bpm, 2) : 'n/a',
      segment.bpmErrorPct === null ? 'n/a' : `${segment.bpmErrorPct.toFixed(2)} %`,
      formatNumber(segment.confidence, 2),
    ];
  });
  const widths = header.map((cell, column) =>
    Math.max(cell.length, ...rows.map((row) => row[column].length)),
  );
  console.log(header.map((cell, column) => (column === 0 ? cell.padEnd(widths[0]) : cell.padStart(widths[column]))).join('  '));
  for (const row of rows) {
    console.log(row.map((cell, column) => (column === 0 ? cell.padEnd(widths[0]) : cell.padStart(widths[column]))).join('  '));
  }
}

function markdownCell(value) {
  return String(value ?? '').replace(/\|/g, '\\|');
}

function summaryMarkdown(options, name, beats, beatRows, tempo, tempoEnabled, beatGateEnabled, pass) {
  const lines = [`### Accuracy: ${name}`, '', `Map: \`${options.map}\``, '', '**Beats**', ''];
  lines.push('| metric | value | threshold |', '| --- | --- | --- |');
  for (const row of beatRows) lines.push(`| ${row.map(markdownCell).join(' | ')} |`);
  lines.push(`| beat gate | ${beatGateEnabled ? verdict(beats.pass) : 'not gated'} | |`, '');
  if (tempoEnabled) {
    const rows = tempoRows(tempo);
    lines.push('**Tempo map**', '', '| metric | value | threshold | status |', '| --- | --- | --- | --- |');
    for (const row of rows) lines.push(`| ${row.map(markdownCell).join(' | ')} |`);
    lines.push(`| tempo-map gate | ${verdict(tempo.pass)} | | |`, '');
  } else {
    lines.push('**Tempo map**', '', 'Gate not enabled for this material.', '');
  }
  lines.push(`**Verdict:** ${verdict(pass)}`, '');
  return lines.join('\n');
}

try {
  const options = parseArgs(process.argv.slice(2));
  const analysis = JSON.parse(fs.readFileSync(options.analysis, 'utf8'));
  const map = JSON.parse(fs.readFileSync(options.map, 'utf8'));
  const name = typeof map.name === 'string' && map.name.trim() ? map.name.trim() : path.basename(options.map, '.json');
  const beatGateEnabled = map.acceptance?.beats ?? true;
  const tempoMapEnabled = map.acceptance?.tempoMap === true;

  const beats = evaluateBeats(analysis, map);
  const tempo = tempoMapEnabled ? evaluateTempoMap(analysis, map) : null;
  const pass = (!beatGateEnabled || beats.pass) && (!tempoMapEnabled || tempo.pass);

  console.log(`material: ${name}`);
  console.log(`map: ${options.map}`);
  console.log('');
  console.log('Beats');
  const beatRows = table(beats);
  printTable(beatRows);
  console.log(`beat gate: ${beatGateEnabled ? verdict(beats.pass) : 'not gated'}`);
  console.log('');
  if (tempoMapEnabled) {
    console.log('Tempo map');
    printSegmentTable(tempo.reported, tempo.truth);
    console.log('');
    printTempoTable(tempoRows(tempo));
    console.log(`tempo-map gate: ${verdict(tempo.pass)}`);
  } else {
    console.log('Tempo map gate: not enabled (acceptance.tempoMap is false)');
  }
  console.log('');
  console.log(`verdict: ${verdict(pass)}`);

  if (options.summary) {
    fs.appendFileSync(
      options.summary,
      `${summaryMarkdown(options, name, beats, beatRows, tempo, tempoMapEnabled, beatGateEnabled, pass)}\n`,
    );
  }
  if (!pass) process.exitCode = 1;
} catch (error) {
  console.error(`score: ${error instanceof Error ? error.message : String(error)}`);
  process.exitCode = 1;
}
