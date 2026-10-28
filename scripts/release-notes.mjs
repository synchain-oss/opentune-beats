#!/usr/bin/env node
// SPDX-FileCopyrightText: Copyright (c) 2026 Synchain
// SPDX-License-Identifier: MIT

// Build the release notes and the SHA256SUMS file of a release from the packed assets.
//
// Usage:
//   node scripts/release-notes.mjs --version <x.y.z> --assets <dir> [--sums FILE] [--out FILE]
//
// The asset names are part of the release contract (docs/output.md, section 5): the two archives
// carry the version, the three model files do not. The script fails when an expected asset is
// missing or when the directory holds a file that is neither an asset nor one of its own output
// files, so a release cannot silently ship a wrong or an extra file.
//
// SHA256SUMS is written in `sha256sum -c` format, with two spaces between the digest and the
// name. The body goes to --out, or to stdout when --out is absent. Release notes are English
// (CLAUDE.md, section 3) and carry no tool attribution; release.yml runs
// scripts/check-attribution.mjs over the body before the release is created.

import { createHash } from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';

/** The assets of a release, in the order they are listed, for a given version. */
function assetNames(version) {
  return [
    `opentune-beats-${version}-win-x64.zip`,
    `opentune-beats-${version}-macos-universal2.tar.gz`,
    'mel_spectrogram.onnx',
    'beat_this_final.onnx',
    'beat_this_small.onnx',
  ];
}

function usage() {
  return 'usage: node scripts/release-notes.mjs --version <x.y.z> --assets <dir> [--sums FILE] [--out FILE]';
}

function parseArgs(argv) {
  const options = { version: '', assets: '', sums: '', out: '' };
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    const key = { '--version': 'version', '--assets': 'assets', '--sums': 'sums', '--out': 'out' }[arg];
    if (!key) throw new Error(`unknown argument ${arg}`);
    const value = argv[index + 1];
    if (value === undefined || value.startsWith('--')) throw new Error(`missing value for ${arg}`);
    options[key] = value;
    index += 1;
  }
  if (!/^\d+\.\d+\.\d+$/.test(options.version)) {
    throw new Error(`--version must be a SemVer release version, got ${JSON.stringify(options.version)}`);
  }
  if (!options.assets) throw new Error('--assets is required');
  return options;
}

const sha256 = (file) => createHash('sha256').update(fs.readFileSync(file)).digest('hex');

/** `1234567` -> `1,234,567`, without depending on a locale database. */
const withSeparators = (value) => String(value).replace(/\B(?=(\d{3})+(?!\d))/g, ',');

function collect(dir, version, ignored) {
  const names = assetNames(version);
  const present = new Set(
    fs
      .readdirSync(dir)
      .filter((name) => fs.statSync(path.join(dir, name)).isFile())
      .filter((name) => !ignored.has(path.resolve(dir, name))),
  );
  const missing = names.filter((name) => !present.has(name));
  if (missing.length) throw new Error(`assets missing from ${dir}: ${missing.join(', ')}`);
  const unexpected = [...present].filter((name) => !names.includes(name)).sort();
  if (unexpected.length) {
    throw new Error(`unexpected files in ${dir}: ${unexpected.join(', ')}`);
  }
  return names.map((name) => {
    const file = path.join(dir, name);
    return { name, size: fs.statSync(file).size, sha256: sha256(file) };
  });
}

function sumsFile(assets) {
  return `${assets.map((asset) => `${asset.sha256}  ${asset.name}`).join('\n')}\n`;
}

function releaseBody(version, assets) {
  const rows = assets
    .map((asset) => `| \`${asset.name}\` | ${withSeparators(asset.size)} | \`${asset.sha256}\` |`)
    .join('\n');
  return `## OpenTune Beats v${version}

Beat, downbeat and tempo-map analysis for backing tracks: a small native command-line tool that
writes one JSON object to stdout. It is the beat-grid engine behind the \`beat-grid\` skill of
OpenTune MCP Server.

### Assets

| Asset | Bytes | SHA-256 |
| --- | ---: | --- |
${rows}

\`SHA256SUMS\` carries the digest of every asset above, in \`sha256sum -c\` format.

### Install

1. Download the archive for the platform: \`opentune-beats-${version}-win-x64.zip\` (Windows x64,
   static C runtime, so no Visual C++ redistributable is needed) or
   \`opentune-beats-${version}-macos-universal2.tar.gz\` (macOS, Apple silicon and Intel in one
   binary).
2. Unpack it. The executable is at the root of the archive, next to \`LICENSE\`, \`NOTICE\` and
   \`THIRD-PARTY-NOTICES\`.
3. Put the models in \`models/\` next to the executable: \`mel_spectrogram.onnx\` and either
   \`beat_this_final.onnx\` (\`--model final\`, the default, about 80 MB) or
   \`beat_this_small.onnx\` (\`--model small\`). The SHA-256 of each file is compiled into the
   binary and checked before the model is loaded; \`opentune-beats version --json\` prints them.
4. \`opentune-beats analyze <backing-track.wav>\` writes the analysis JSON to stdout. Logs and
   progress go to stderr.

Check the downloads with \`sha256sum -c SHA256SUMS\`, or \`shasum -a 256 -c SHA256SUMS\` on macOS.

### License

OpenTune Beats is MIT licensed; see \`LICENSE\`. The Beat This! code and the beat_this model
weights are MIT licensed. Part of the data the models were trained on is copyrighted or under
restricted Creative Commons licences, which does not affect the MIT licence of the weights; see
\`NOTICE\`. The licences and copyright lines of the Rust crates compiled into the binaries are
reproduced in \`THIRD-PARTY-NOTICES\`, which ships in both archives.
`;
}

function main(argv) {
  let options;
  try {
    options = parseArgs(argv);
  } catch (error) {
    process.stderr.write(`release-notes: ${error.message}\n${usage()}\n`);
    return 2;
  }

  const dir = path.resolve(options.assets);
  const sums = path.resolve(options.sums || path.join(dir, 'SHA256SUMS'));
  const out = options.out ? path.resolve(options.out) : '';
  // This script's own output is not an asset, whether it is written here or elsewhere: a second
  // run over the same directory (the release job re-checks the downloaded artifact) must produce
  // the same list.
  const ignored = new Set([
    sums,
    path.join(dir, 'SHA256SUMS'),
    path.join(dir, 'release-notes.md'),
    ...(out ? [out] : []),
  ]);

  try {
    const assets = collect(dir, options.version, ignored);
    fs.writeFileSync(sums, sumsFile(assets));
    const body = releaseBody(options.version, assets);
    if (out) {
      fs.writeFileSync(out, body);
      process.stderr.write(`wrote ${out} and ${sums} (${assets.length} assets)\n`);
    } else {
      process.stdout.write(body);
      process.stderr.write(`wrote ${sums} (${assets.length} assets)\n`);
    }
    return 0;
  } catch (error) {
    process.stderr.write(`release-notes: ${error instanceof Error ? error.message : String(error)}\n`);
    return 1;
  }
}

process.exitCode = main(process.argv.slice(2));
