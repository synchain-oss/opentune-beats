#!/usr/bin/env node
// check-attribution — the single implementation of the "zero AI attribution" check: commit
// messages, pull request text and repository files must not credit an AI tool, and commits
// carry a DCO sign-off. Self-contained (Node >= 22, no dependencies) so that the MCP repo
// and the fork can carry byte-identical copies next to attribution-patterns.txt.
//
// Library:
//   import { checkText, loadPatterns, stripCommitMessage } from './check-attribution.mjs'
//   checkText(text, { requireSignoff, signoffEmails, commitMessage }) -> { ok, violations }
// CLI:
//   node check-attribution.mjs [options] [FILE|-]...
//     --require-signoff        require a Signed-off-by trailer
//     --signoff-email <email>  (repeatable) Signed-off-by email must be one of these
//     --commit-msg             strip '#' comment lines and the scissors section first
//     --comment-char <c>       comment char for --commit-msg (default '#')
//     --patterns <file>        alternative patterns file
//     --text <string>          check a literal string (repeatable)
//     --json                   machine-readable output
//   exit 0 = clean, 1 = violations, 2 = usage or I/O error
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const HERE = path.dirname(fileURLToPath(import.meta.url));
export const DEFAULT_PATTERNS_FILE = path.join(HERE, 'attribution-patterns.txt');

const ZERO_WIDTH = /[­᠎​-‏‪-‮⁠-⁤﻿]/g;
const SIGNOFF_RE = /^\s*signed-off-by\s*:\s*(.*?)\s*<([^<>\s]+)>\s*$/i;

let cachedPatterns = null;

export function parsePatterns(text) {
  const out = [];
  for (const [n, rawLine] of text.split(/\r?\n/).entries()) {
    const line = rawLine.trim();
    if (!line || line.startsWith('#')) continue;
    const m = /^(\S+)\s+(.+)$/.exec(line);
    if (!m) throw new Error(`attribution-patterns: malformed line ${n + 1}: ${rawLine}`);
    out.push({ id: m[1], re: new RegExp(m[2], 'iu') });
  }
  if (!out.length) throw new Error('attribution-patterns: no patterns');
  return out;
}

export function loadPatterns(file = DEFAULT_PATTERNS_FILE) {
  if (file === DEFAULT_PATTERNS_FILE && cachedPatterns) return cachedPatterns;
  const pats = parsePatterns(fs.readFileSync(file, 'utf8'));
  if (file === DEFAULT_PATTERNS_FILE) cachedPatterns = pats;
  return pats;
}

export function normalizeText(text) {
  return String(text ?? '').normalize('NFKC').replace(ZERO_WIDTH, '');
}

// Remove what git strips from a commit message: comment lines and everything below
// the scissors line written by `git commit -v`.
export function stripCommitMessage(text, commentChar = '#') {
  const lines = String(text ?? '').split(/\r?\n/);
  const out = [];
  for (const line of lines) {
    if (line.startsWith(`${commentChar} ------------------------ >8 ------------------------`)) break;
    if (line.startsWith(commentChar)) continue;
    out.push(line);
  }
  return out.join('\n');
}

export function findSignoffs(text) {
  const res = [];
  for (const line of normalizeText(text).split(/\r?\n/)) {
    const m = SIGNOFF_RE.exec(line);
    if (m) res.push({ name: m[1], email: m[2].toLowerCase() });
  }
  return res;
}

/**
 * @param {string} text
 * @param {{requireSignoff?: boolean, signoffEmails?: string[]|null, commitMessage?: boolean,
 *          commentChar?: string, patterns?: {id:string,re:RegExp}[], source?: string}} [opts]
 */
export function checkText(text, opts = {}) {
  const patterns = opts.patterns ?? loadPatterns();
  let body = String(text ?? '');
  if (opts.commitMessage) body = stripCommitMessage(body, opts.commentChar ?? '#');
  const norm = normalizeText(body);
  const violations = [];
  norm.split(/\r?\n/).forEach((line, idx) => {
    for (const p of patterns) {
      if (p.re.test(line)) violations.push({ rule: p.id, line: idx + 1, text: line.trim().slice(0, 200), source: opts.source ?? null });
    }
  });
  if (opts.requireSignoff) {
    const signoffs = findSignoffs(norm);
    const allowed = opts.signoffEmails?.length ? new Set(opts.signoffEmails.map((e) => e.toLowerCase())) : null;
    if (!signoffs.length) violations.push({ rule: 'missing-signoff', line: 0, text: 'no Signed-off-by trailer (use git commit -s)', source: opts.source ?? null });
    else if (allowed && !signoffs.some((s) => allowed.has(s.email))) {
      violations.push({ rule: 'signoff-identity', line: 0, text: `Signed-off-by email not allowed: ${signoffs.map((s) => s.email).join(', ')}`, source: opts.source ?? null });
    }
  }
  return { ok: violations.length === 0, violations };
}

export function formatViolations(violations) {
  return violations
    .map((v) => `${v.source ? `${v.source}:` : ''}${v.line ? `${v.line}: ` : ' '}[${v.rule}] ${v.text}`)
    .join('\n');
}

function parseArgs(argv) {
  const o = { files: [], texts: [], signoffEmails: [], requireSignoff: false, commitMessage: false, commentChar: '#', patterns: null, json: false };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    const val = () => {
      if (i + 1 >= argv.length) throw new Error(`missing value for ${a}`);
      return argv[++i];
    };
    if (a === '--require-signoff') o.requireSignoff = true;
    else if (a === '--signoff-email') o.signoffEmails.push(val());
    else if (a === '--commit-msg') o.commitMessage = true;
    else if (a === '--comment-char') o.commentChar = val();
    else if (a === '--patterns') o.patterns = val();
    else if (a === '--text') o.texts.push(val());
    else if (a === '--json') o.json = true;
    else if (a === '-h' || a === '--help') o.help = true;
    else if (a.startsWith('--')) throw new Error(`unknown option ${a}`);
    else o.files.push(a);
  }
  return o;
}

function main(argv) {
  let o;
  try {
    o = parseArgs(argv);
  } catch (e) {
    process.stderr.write(`check-attribution: ${e.message}\n`);
    return 2;
  }
  if (o.help || (!o.files.length && !o.texts.length)) {
    process.stderr.write('usage: check-attribution [--require-signoff] [--signoff-email E] [--commit-msg] [--patterns F] [--json] [--text S] FILE|-...\n');
    return 2;
  }
  let patterns;
  try {
    patterns = loadPatterns(o.patterns ? path.resolve(o.patterns) : DEFAULT_PATTERNS_FILE);
  } catch (e) {
    process.stderr.write(`check-attribution: ${e.message}\n`);
    return 2;
  }
  const all = [];
  const inputs = [...o.texts.map((t, i) => ({ source: `--text#${i + 1}`, text: t }))];
  for (const f of o.files) {
    try {
      inputs.push({ source: f === '-' ? '<stdin>' : f, text: fs.readFileSync(f === '-' ? 0 : f, 'utf8') });
    } catch (e) {
      process.stderr.write(`check-attribution: cannot read ${f}: ${e.message}\n`);
      return 2;
    }
  }
  for (const inp of inputs) {
    const r = checkText(inp.text, { patterns, requireSignoff: o.requireSignoff, signoffEmails: o.signoffEmails, commitMessage: o.commitMessage, commentChar: o.commentChar, source: inp.source });
    all.push(...r.violations);
  }
  if (o.json) process.stdout.write(JSON.stringify({ ok: all.length === 0, violations: all }, null, 2) + '\n');
  else if (all.length) process.stderr.write(`check-attribution: ${all.length} violation(s)\n${formatViolations(all)}\n`);
  return all.length ? 1 : 0;
}

const isMain = process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url);
if (isMain) process.exitCode = main(process.argv.slice(2));
