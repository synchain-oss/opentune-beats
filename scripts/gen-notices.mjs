#!/usr/bin/env node
// SPDX-FileCopyrightText: Copyright (c) 2026 Synchain
// SPDX-License-Identifier: MIT

// Generate THIRD-PARTY-NOTICES: for every crate the release targets compile, the license files
// and copyright lines from the crate package itself.
//
// Usage: node scripts/gen-notices.mjs [--check] [--out FILE] [--target TRIPLE]...
//
// The crate list is the union of the dependency graphs cargo resolves for the release targets:
// Windows x64 and both macOS architectures (the two slices of the universal2 binary). It is not
// the Cargo.lock package list, which also pins crates only other targets (wasm32, Hermit) use.
//
// License texts are read from the unpacked packages under the local Cargo registry; nothing is
// downloaded. Run `cargo fetch --locked` first on a machine whose registry is empty (the script
// asks cargo for the graph with `--offline`, so a missing package is an error, not a download).
// `CARGO` overrides which cargo binary is used. A package that ships no license file is listed
// with the SPDX standard text from LICENSES/ and the entry says so; a package that cannot be read
// is an error, never a silent omission.
//
// --check regenerates the file in memory and exits 1 when it differs from the tracked file. CI
// runs it, so the checked-in notices and the dependency graph cannot drift apart.
//
// Exit codes: 0 clean, 1 out of date (--check only), 2 a generation or usage error.

import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const DEFAULT_OUT = 'THIRD-PARTY-NOTICES';
const DEFAULT_TARGETS = ['x86_64-pc-windows-msvc', 'aarch64-apple-darwin', 'x86_64-apple-darwin'];
const SPDX_DIR = 'LICENSES';
// The standard texts this repository carries. A crate that ships no license file can only be
// listed when its declared license is in this map; anything else is a generation error, so a new
// fallback license is a deliberate addition, not an empty entry.
const STANDARD_TEXTS = new Map([
  ['MIT', 'MIT.txt'],
  ['Apache-2.0', 'Apache-2.0.txt'],
]);

function usage() {
  return 'usage: node scripts/gen-notices.mjs [--check] [--out FILE] [--target TRIPLE]...';
}

function parseArgs(argv) {
  const options = { check: false, out: DEFAULT_OUT, targets: [] };
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (arg === '--check') {
      options.check = true;
    } else if (arg === '--out' || arg === '--target') {
      const value = argv[index + 1];
      if (value === undefined || value.startsWith('--')) throw new Error(`missing value for ${arg}`);
      if (arg === '--out') options.out = value;
      else options.targets.push(value);
      index += 1;
    } else {
      throw new Error(`unknown argument ${arg}`);
    }
  }
  if (!options.targets.length) options.targets = [...DEFAULT_TARGETS];
  return options;
}

/**
 * Runs cargo and returns its standard output. The output goes to files instead of a pipe: a
 * dependency graph is large, and a captured pipe that nothing drains can block the child.
 * @param {string[]} args
 * @returns {string}
 */
function runCargo(args) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'gen-notices-'));
  const outFile = path.join(dir, 'stdout');
  const errFile = path.join(dir, 'stderr');
  try {
    const out = fs.openSync(outFile, 'w');
    const err = fs.openSync(errFile, 'w');
    let result;
    try {
      result = spawnSync(process.env.CARGO || 'cargo', args, { cwd: ROOT, stdio: ['ignore', out, err] });
    } finally {
      fs.closeSync(out);
      fs.closeSync(err);
    }
    if (result.error) {
      throw new Error(`cannot run ${process.env.CARGO || 'cargo'}: ${result.error.message}`);
    }
    if (result.status !== 0) {
      const detail = fs.readFileSync(errFile, 'utf8').trim();
      throw new Error(`cargo ${args.join(' ')} failed${detail ? `: ${detail}` : ` (exit ${result.status})`}`);
    }
    return fs.readFileSync(outFile, 'utf8');
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
}

/**
 * The resolved dependency graph for one target.
 * @param {string} target
 * @returns {{ packages: any[], resolve: { root: string, nodes: { id: string }[] } }}
 */
function metadataFor(target) {
  const args = [
    'metadata',
    '--locked',
    '--offline',
    '--format-version',
    '1',
    '--filter-platform',
    target,
    '--manifest-path',
    path.join(ROOT, 'Cargo.toml'),
  ];
  try {
    return JSON.parse(runCargo(args));
  } catch (error) {
    throw new Error(`${error.message} (if the registry is not populated, run "cargo fetch --locked")`);
  }
}

/** License and notice files at the root of a crate package, in a stable order. */
function packageFiles(dir) {
  const names = fs.readdirSync(dir).filter((name) => /^(?:LICEN[CS]E|COPYING|NOTICE)/i.test(name));
  names.sort((left, right) => (left < right ? -1 : left > right ? 1 : 0));
  return names.map((name) => {
    const file = path.join(dir, name);
    if (!fs.statSync(file).isFile()) return { name, text: null };
    const text = fs.readFileSync(file, 'utf8').replace(/\r\n?/g, '\n').replace(/\n+$/, '');
    if (text.includes('\0')) throw new Error(`${name}: not a text file`);
    return { name, text };
  });
}

const isLicenseFile = (name) => /^(?:LICEN[CS]E|COPYING)/i.test(name);
const isNoticeFile = (name) => /^NOTICE/i.test(name);

/** Separate SPDX license ids from an expression such as `(MIT OR Apache-2.0) AND Unicode-3.0`. */
function licenseIds(expression) {
  const ids = [];
  for (const token of expression.replace(/[()]/g, ' ').split(/[\s/]+/)) {
    if (!token || token === 'AND' || token === 'OR') continue;
    if (token === 'WITH') continue;
    if (!ids.includes(token)) ids.push(token);
  }
  return ids;
}

// A copyright notice starts with "Copyright" or "(c)" as a word of its own; the lowercase
// "copyright notice ..." lines of the Apache definitions do not. The appendix of the Apache text
// ("Copyright {yyyy} {name of copyright owner}") is a template, not a notice.
const COPYRIGHT_RE = /^\s*(?:Copyright\b|©)/;
const COPYRIGHT_TEMPLATE_RE = /\{yyyy\}|\[yyyy\]|\{name of copyright owner\}|\[name of copyright owner\]/i;

/**
 * Copyright lines stated by the package's own license and notice files, one per unique line,
 * with the file that states it. The MIT and Apache boilerplate ("THE AUTHORS OR COPYRIGHT
 * HOLDERS ...") is not a copyright line and does not match.
 * @param {{ name: string, text: string | null }[]} files
 * @returns {{ text: string, files: string[] }[]}
 */
function copyrightLines(files) {
  const found = [];
  for (const file of files) {
    if (file.text === null) continue;
    for (const line of file.text.split('\n')) {
      if (!COPYRIGHT_RE.test(line) || COPYRIGHT_TEMPLATE_RE.test(line)) continue;
      const text = line.trim().replace(/\s+/g, ' ');
      const entry = found.find((item) => item.text === text);
      if (entry) {
        if (!entry.files.includes(file.name)) entry.files.push(file.name);
      } else {
        found.push({ text, files: [file.name] });
      }
    }
  }
  return found;
}

/** A vendored SPDX standard text, normalized the same way as a package file. */
function standardText(licenseId) {
  const name = STANDARD_TEXTS.get(licenseId);
  if (!name) {
    throw new Error(
      `no SPDX standard text for ${JSON.stringify(licenseId)}: add ${SPDX_DIR}/<id>.txt and a ` +
        'STANDARD_TEXTS entry for it',
    );
  }
  const file = path.join(ROOT, SPDX_DIR, name);
  const text = fs.readFileSync(file, 'utf8').replace(/\r\n?/g, '\n').replace(/\n+$/, '');
  return { name, text };
}

function compareCrates(left, right) {
  if (left.name !== right.name) return left.name < right.name ? -1 : 1;
  if (left.version !== right.version) return left.version < right.version ? -1 : 1;
  return 0;
}

/**
 * Builds the notices file. The result depends only on Cargo.toml, Cargo.lock and the crate
 * packages, so two machines with the same lock generate the same bytes.
 * @param {{ targets: string[] }} options
 * @returns {{ text: string, crates: { name: string, version: string }[] }}
 */
function build({ targets }) {
  const meta = metadataFor(targets[0]);
  const rootName = meta.packages.find((pkg) => pkg.id === meta.resolve.root)?.name ?? 'opentune-beats';
  const packages = new Map();
  const inGraph = new Set();
  for (const target of targets) {
    const result = target === targets[0] ? meta : metadataFor(target);
    for (const pkg of result.packages) packages.set(pkg.id, pkg);
    for (const node of result.resolve.nodes) inGraph.add(node.id);
  }

  const crates = [...inGraph]
    .map((id) => packages.get(id))
    .filter((pkg) => pkg !== undefined && pkg.name !== rootName)
    .sort(compareCrates);
  if (!crates.length) throw new Error('the resolved graph has no third-party crates');

  const entries = [];
  for (const pkg of crates) {
    const dir = path.dirname(pkg.manifest_path);
    if (!fs.existsSync(dir)) {
      throw new Error(
        `${pkg.name} ${pkg.version}: the crate source is not unpacked (run "cargo fetch --locked")`,
      );
    }
    const license = typeof pkg.license === 'string' ? pkg.license.trim() : '';
    if (!license) throw new Error(`${pkg.name} ${pkg.version}: no license declared in Cargo.toml`);

    const files = packageFiles(dir);
    const licenseFiles = files.filter((file) => isLicenseFile(file.name));
    const noticeFiles = files.filter((file) => isNoticeFile(file.name));
    const upstream = pkg.repository || `https://crates.io/crates/${pkg.name}/${pkg.version}`;

    const lines = [
      '-'.repeat(80),
      `${pkg.name} ${pkg.version}`,
      `Declared license: ${license}`,
      `Upstream: ${upstream}`,
      '',
    ];

    const copyrights = copyrightLines([...licenseFiles, ...noticeFiles]);
    if (copyrights.length) {
      lines.push('Copyright:');
      for (const item of copyrights) lines.push(`  ${item.text} (${item.files.join(', ')})`);
    } else {
      lines.push('Copyright: no copyright line in the published crate package; see the upstream entry above.');
    }

    if (licenseFiles.length === 0) {
      lines.push(
        'License text: the package ships no license file, so the SPDX standard text for its',
        'declared license is reproduced below from LICENSES/.',
      );
      for (const id of licenseIds(license)) {
        const standard = standardText(id);
        lines.push(
          '',
          `----- ${id} (SPDX standard text, ${SPDX_DIR}/${standard.name}) -----`,
          standard.text,
        );
      }
    }

    for (const file of [...licenseFiles, ...noticeFiles]) {
      if (file.text === null) {
        throw new Error(`${pkg.name} ${pkg.version}: ${file.name} is not a regular file`);
      }
      lines.push('', `----- ${file.name} -----`, file.text);
    }
    lines.push('');
    entries.push(lines.join('\n'));
  }

  const header = [
    'OpenTune Beats - third-party notices',
    '',
    'OpenTune Beats is MIT licensed; see LICENSE. This file reproduces the licenses and copyright',
    'lines of the third-party Rust crates that can be compiled into a release binary.',
    '',
    'How this file is generated',
    '  scripts/gen-notices.mjs resolves the dependency graph with `cargo metadata',
    '  --filter-platform` for the release targets and reads the license files from each crate',
    '  package in the local Cargo registry. The output is deterministic, and CI regenerates it:',
    '  the checked-in file must be byte-identical (`node scripts/gen-notices.mjs --check`).',
    '',
    'Which crates are listed',
    `  The targets are ${targets.join(', ')}.`,
    '  A crate is listed once when any of those targets compiles it, build dependencies and',
    '  proc-macros included. Crates that only another target compiles (wasm32, Hermit and the',
    '  like) are not listed. This is not an exhaustive inventory of every version in Cargo.lock.',
    '',
    'License policy',
    '  This file records the licenses; it is not the project\'s license policy. CI runs',
    '  `cargo deny check licenses` (deny.toml) over the same release targets as an independent',
    '  check, and a crate whose license falls outside the policy fails that check instead of',
    '  being left out of this file.',
    '',
    'License texts',
    '  Every LICENSE, COPYING and NOTICE file at the root of a crate package is reproduced',
    '  verbatim, except that line endings are normalized to LF. A package that ships no license',
    '  file is marked in its entry, and the SPDX standard text for its declared license is',
    '  reproduced from LICENSES/ in this repository.',
    '',
    'Beat This! and beat-this-rs are described in NOTICE.',
    '',
    'Crates',
    '',
    '',
  ].join('\n');

  return { text: `${header}\n${entries.join('\n')}`, crates };
}

/** Reports where two versions of the file first differ, without printing a large diff. */
function reportDifference(expected, actual) {
  const wanted = expected.split('\n');
  const have = actual.split('\n');
  const limit = Math.max(wanted.length, have.length);
  for (let i = 0; i < limit; i += 1) {
    if (wanted[i] !== have[i]) {
      process.stderr.write(`first difference at line ${i + 1}:\n`);
      process.stderr.write(`  generated: ${wanted[i] === undefined ? '(end of file)' : JSON.stringify(wanted[i])}\n`);
      process.stderr.write(`  on disk:   ${have[i] === undefined ? '(end of file)' : JSON.stringify(have[i])}\n`);
      break;
    }
  }
  process.stderr.write(
    `generated ${wanted.length} lines, ${Buffer.byteLength(expected)} bytes; ` +
      `on disk ${have.length} lines, ${Buffer.byteLength(actual)} bytes\n`,
  );
}

function main(argv) {
  let options;
  try {
    options = parseArgs(argv);
  } catch (error) {
    process.stderr.write(`gen-notices: ${error.message}\n${usage()}\n`);
    return 2;
  }

  let built;
  try {
    built = build(options);
  } catch (error) {
    process.stderr.write(`gen-notices: ${error instanceof Error ? error.message : String(error)}\n`);
    return 2;
  }

  const out = path.resolve(ROOT, options.out);
  if (options.check) {
    let actual;
    try {
      actual = fs.readFileSync(out, 'utf8');
    } catch (error) {
      process.stderr.write(`gen-notices: cannot read ${options.out}: ${error.message}\n`);
      return 1;
    }
    if (actual === built.text) {
      process.stdout.write(`${options.out}: up to date (${built.crates.length} crates)\n`);
      return 0;
    }
    process.stderr.write(`gen-notices: ${options.out} is out of date\n`);
    reportDifference(built.text, actual);
    process.stderr.write('regenerate with: node scripts/gen-notices.mjs\n');
    return 1;
  }

  fs.writeFileSync(out, built.text);
  process.stdout.write(
    `wrote ${options.out}: ${built.crates.length} crates, ${Buffer.byteLength(built.text)} bytes\n`,
  );
  return 0;
}

process.exitCode = main(process.argv.slice(2));
