#!/usr/bin/env node
// SPDX-FileCopyrightText: Copyright (c) 2026 Synchain
// SPDX-License-Identifier: MIT

// CI checks that need the GitHub event context: .github/workflows/ci.yml (compliance job) and
// .github/workflows/branch-gate.yml. Each subcommand reads its context from environment
// variables (the workflows pass them through `env:`, never by expanding `${{ }}` inside a
// script) and exits 0 when clean, 1 when it found problems, 2 on a usage or context error.
//
// Usage: node scripts/ci-checks.mjs <check>
//   scope                 the commit range and identity rules of this event, printed as
//                         base=…, allow_external_authors=…, allow_dependabot=… for $GITHUB_OUTPUT
//                         (env: EVENT_NAME, BASE_REF, BEFORE, DEFAULT_BRANCH, REPO, HEAD_REPO,
//                         HEAD_REF, PR_AUTHOR)
//   branch                source branch rules of a pull request into dev or prod
//                         (env: BASE_REF, HEAD_REF, HEAD_REPO, REPO)
//   commits               sign-off, attribution and identity of the commits in <base>..HEAD
//                         (env: as for scope)
//   files                 AI attribution in every text file of the repository
//   privacy               personal paths and e-mail addresses outside the project's own
//   assets                audio and model files, by extension and by signature
//   pull-request-target   the pull_request_target ban in .github/workflows/
//
// Needs only Node (>= 22) and git: it imports nothing outside scripts/ and installs nothing.

import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

import { checkText, loadPatterns } from './check-attribution.mjs';

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const PATTERNS_FILE = 'scripts/attribution-patterns.txt';

const MAINTAINER_NAME = 'DLsnows';

/** The project's own addresses, as author or committer of a local commit. */
const PROJECT_EMAILS = Object.freeze(['noreply@synchain.ca', 'contact@synchain.ca']);

/**
 * The maintainer's GitHub noreply address: GitHub's web merge records it as the author of the
 * commits it creates; a local commit never uses it. It is also the only personal-looking
 * address a repository file may carry (it is a project identity, not a mailbox).
 */
const MAINTAINER_GITHUB_NOREPLY = ['140901583+dlsnows', 'users.noreply.github.com'].join('@');

/** The committer GitHub records for a merge, squash or rebase done on the web. */
const WEB_MERGE_NAME = 'GitHub';
const WEB_MERGE_EMAIL = ['noreply', 'github.com'].join('@');

/** Dependabot's commit identity. */
const DEPENDABOT_NAME = 'dependabot[bot]';
const DEPENDABOT_EMAIL = ['49699333+dependabot[bot]', 'users.noreply.github.com'].join('@');

/** Addresses the privacy scan accepts in a file. */
const ALLOWED_EMAILS = Object.freeze([...PROJECT_EMAILS, MAINTAINER_GITHUB_NOREPLY]);

const AUDIO_EXTENSIONS = Object.freeze([
  '.aac',
  '.aif',
  '.aifc',
  '.aiff',
  '.amr',
  '.ape',
  '.au',
  '.bwf',
  '.caf',
  '.flac',
  '.kar',
  '.m4a',
  '.m4b',
  '.mka',
  '.mid',
  '.midi',
  '.mp2',
  '.mp3',
  '.oga',
  '.ogg',
  '.opus',
  '.rf64',
  '.snd',
  '.w64',
  '.wav',
  '.wave',
  '.wma',
  '.wv',
]);

const MODEL_EXTENSIONS = Object.freeze([
  '.ckpt',
  '.ggml',
  '.gguf',
  '.mlmodel',
  '.onnx',
  '.ot',
  '.pt',
  '.pth',
  '.safetensors',
  '.tflite',
]);

function log(message) {
  process.stderr.write(`${message}\n`);
}

function env(name, { required = true } = {}) {
  const value = process.env[name] ?? '';
  if (required && value === '') throw new Error(`environment variable ${name} is not set`);
  return value;
}

/**
 * Runs git and returns its standard output. The output goes to temporary files rather than a
 * pipe: a long `git log` cannot fill a buffer that nothing is draining, and the call works the
 * same in every environment the checks run in.
 */
function git(args) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'ci-checks-'));
  const outFile = path.join(dir, 'stdout');
  const errFile = path.join(dir, 'stderr');
  try {
    const out = fs.openSync(outFile, 'w');
    const err = fs.openSync(errFile, 'w');
    let status;
    try {
      status = spawnSync('git', args, { cwd: ROOT, stdio: ['ignore', out, err] }).status;
    } finally {
      fs.closeSync(out);
      fs.closeSync(err);
    }
    if (status !== 0) {
      const detail = fs.readFileSync(errFile, 'utf8').trim();
      throw new Error(`git ${args.join(' ')} failed${detail ? `: ${detail}` : ` (exit ${status})`}`);
    }
    return fs.readFileSync(outFile, 'utf8');
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
}

/** True when the commit is in this clone. `sha` must already be a full hexadecimal object name. */
function commitExists(sha) {
  const result = spawnSync('git', ['cat-file', '-e', `${sha}^{commit}`], { cwd: ROOT, stdio: 'ignore' });
  return result.status === 0;
}

/**
 * Every file that belongs to the repository, as repository-relative paths with forward slashes:
 * the files in the index plus the untracked files that are not ignored.
 *
 * `git ls-files` is the authority, so a file `.gitignore` covers (a local `models/`, `target/`,
 * `.cache/`) is not scanned, and a file that is in the index but deleted from disk is skipped.
 * A missing git or a git failure throws, so the check fails instead of scanning nothing.
 * @returns {string[]}
 */
function repoFiles() {
  const listed = git(['ls-files', '-z', '--cached', '--others', '--exclude-standard']);
  const files = new Set();
  for (const rel of listed.split('\0')) {
    if (!rel) continue;
    try {
      if (!fs.statSync(path.join(ROOT, rel)).isFile()) continue;
    } catch (error) {
      if (error.code === 'ENOENT') continue;
      throw error;
    }
    files.add(rel);
  }
  return [...files].sort();
}

/** Files larger than this are reported instead of scanned, so nothing is skipped silently. */
const MAX_TEXT_BYTES = 2 * 1024 * 1024;

/**
 * Reads a file for the text checks.
 * @returns {{ text: string } | { binary: true } | { tooLarge: number }}
 */
function readText(rel) {
  const file = path.join(ROOT, rel);
  const size = fs.statSync(file).size;
  if (size > MAX_TEXT_BYTES) return { tooLarge: size };
  const buffer = fs.readFileSync(file);
  // A NUL byte in the first 8 KiB is the usual marker of a binary file; the assets check owns
  // binary files.
  if (buffer.subarray(0, 8192).includes(0)) return { binary: true };
  return { text: buffer.toString('utf8') };
}

/**
 * The text files, plus one problem per file that is too large to scan.
 * @returns {{ files: { path: string, text: string }[], problems: string[] }}
 */
function textFiles() {
  const files = [];
  const problems = [];
  for (const rel of repoFiles()) {
    const result = readText(rel);
    if (result.text !== undefined) {
      files.push({ path: rel, text: result.text });
    } else if (result.tooLarge !== undefined) {
      problems.push(`${rel}: file too large to scan (${result.tooLarge} bytes, limit ${MAX_TEXT_BYTES})`);
    }
  }
  return { files, problems };
}

/** The first bytes of a file, for signature checks. */
function head(rel, bytes = 12) {
  const buffer = Buffer.alloc(bytes);
  const fd = fs.openSync(path.join(ROOT, rel), 'r');
  try {
    return buffer.subarray(0, fs.readSync(fd, buffer, 0, bytes, 0));
  } finally {
    fs.closeSync(fd);
  }
}

// ---------------------------------------------------------------------------------------------
// Personal data

// Placeholder account names: a home directory under one of these names names no person.
const NEUTRAL_USERS = new Set(['public', 'default', 'all', 'runner', 'runneradmin', 'node', 'user', 'username']);

// A user name starts with a letter, digit or underscore and stops at a separator, whitespace or
// a character only placeholders use, so `<name>`, `%USERNAME%`, `$HOME`, `{user}` never match.
const NAME = String.raw`([\p{L}\p{N}_][^\\/\s"'\x60<>|:*?%$\{\}\[\]()]*)`;
// One or more path separators of either kind (JSON and source strings double the backslash).
const SEP = String.raw`(?:\\+|/+)`;

// The home-directory patterns. Each one is built from the parts above so that this file does not
// match itself. The look-behinds skip URL paths such as `example.com/Users/<name>`.
const HOME_PATTERNS = Object.freeze([
  new RegExp(String.raw`(?<![A-Za-z])[A-Za-z]:${SEP}Users${SEP}${NAME}`, 'giu'),
  new RegExp(String.raw`(?<![A-Za-z0-9.:\-])/(?:mnt/|cygdrive/)?[A-Za-z]/Users/${NAME}`, 'giu'),
  new RegExp(String.raw`(?<![A-Za-z0-9.:\-])/Users/${NAME}`, 'gu'),
  new RegExp(String.raw`(?<![A-Za-z0-9.:\-])/home/${NAME}`, 'gu'),
  new RegExp(String.raw`(?:\\{2,}|/{2,})wsl(?:\$|\.localhost)${SEP}[^\\/\s]+${SEP}home${SEP}${NAME}`, 'giu'),
]);

const EMAIL_RE = /[A-Za-z0-9._%+-]+@[A-Za-z0-9-]+(?:\.[A-Za-z0-9-]+)*\.[A-Za-z]{2,}/g;

const isNeutral = (user) => NEUTRAL_USERS.has(user.toLowerCase());

/**
 * Personal paths (a home directory that names a person) and e-mail addresses other than the
 * project's own.
 * @param {string} text
 * @param {string} source path shown in the report
 * @param {{ allowEmails?: string[] }} [options]
 * @returns {string[]}
 */
function scanPrivacy(text, source, { allowEmails = [] } = {}) {
  const problems = [];
  text.split('\n').forEach((line, i) => {
    const where = `${source}:${i + 1}`;
    for (const re of HOME_PATTERNS) {
      for (const m of line.matchAll(re)) {
        if (!isNeutral(m[1])) problems.push(`${where}: personal path "${m[0]}"`);
      }
    }
    for (const m of line.matchAll(EMAIL_RE)) {
      const email = m[0].toLowerCase();
      if (!ALLOWED_EMAILS.includes(email) && !allowEmails.includes(email)) {
        problems.push(`${where}: e-mail address "${m[0]}" is not a project address`);
      }
    }
  });
  return problems;
}

// ---------------------------------------------------------------------------------------------
// Audio and model files

/** Signatures at the start of common audio files, tightened so that text does not match. */
const AUDIO_SIGNATURES = Object.freeze([
  {
    format: 'WAV',
    test: (b) => ['RIFF', 'RF64', 'BW64'].some((magic) => hasAscii(b, 0, magic)) && hasAscii(b, 8, 'WAVE'),
  },
  { format: 'AIFF', test: (b) => hasAscii(b, 0, 'FORM') && (hasAscii(b, 8, 'AIFF') || hasAscii(b, 8, 'AIFC')) },
  { format: 'FLAC', test: (b) => hasAscii(b, 0, 'fLaC') && ((b[4] ?? 0xff) & 0x7f) === 0 },
  { format: 'Ogg', test: (b) => hasAscii(b, 0, 'OggS') && b[4] === 0 },
  { format: 'MP3 (ID3 tag)', test: (b) => hasAscii(b, 0, 'ID3') && [2, 3, 4].includes(b[3] ?? -1) },
  { format: 'MPEG-4 audio', test: (b) => hasAscii(b, 4, 'ftyp') && hasAscii(b, 8, 'M4A ') },
  { format: 'CAF', test: (b) => hasAscii(b, 0, 'caff') && b[4] === 0 && b[5] === 1 },
  { format: 'WavPack', test: (b) => hasAscii(b, 0, 'wvpk') },
  { format: 'AMR', test: (b) => hasAscii(b, 0, '#!AMR\n') || hasAscii(b, 0, '#!AMR-WB\n') },
  { format: 'MIDI', test: (b) => hasAscii(b, 0, 'MThd') && b[4] === 0 && b[5] === 0 },
]);

function hasAscii(bytes, offset, text) {
  return [...text].every((ch, i) => bytes[offset + i] === ch.charCodeAt(0));
}

/**
 * Why a file may not be in the repository: an audio or model extension, or an audio signature in
 * its first bytes (so a renamed recording still counts). Null for anything else.
 * @param {string} rel
 * @param {Uint8Array} firstBytes
 * @returns {string | null}
 */
function forbiddenKind(rel, firstBytes) {
  const name = rel.slice(rel.lastIndexOf('/') + 1).toLowerCase();
  const dot = name.lastIndexOf('.');
  const extension = dot > 0 ? name.slice(dot) : '';
  if (AUDIO_EXTENSIONS.includes(extension)) return `audio file extension ${extension}`;
  if (MODEL_EXTENSIONS.includes(extension)) return `model file extension ${extension}`;
  const signature = AUDIO_SIGNATURES.find(({ test }) => test(firstBytes));
  return signature ? `${signature.format} data` : null;
}

// ---------------------------------------------------------------------------------------------
// Commits

/** `git log --format` read by parseGitLog: identities, then the message. */
const GIT_LOG_FORMAT = '%H%x1f%P%x1f%an%x1f%ae%x1f%cn%x1f%ce%x1f%B%x1e';

/**
 * @param {string} text
 * @returns {{ sha: string, parents: string[], name: string, email: string, committerName: string,
 *   committerEmail: string, body: string }[]}
 */
function parseGitLog(text) {
  return text
    .split('\x1e')
    .map((record) => record.replace(/^\n+|\n+$/g, ''))
    .filter(Boolean)
    .map((record) => {
      const [sha = '', parents = '', name = '', email = '', committerName = '', committerEmail = '', body = ''] =
        record.split('\x1f');
      return { sha, parents: parents.split(/\s+/).filter(Boolean), name, email, committerName, committerEmail, body };
    });
}

const isMaintainerName = (name) => name.toLowerCase() === MAINTAINER_NAME.toLowerCase();

/** A commit Dependabot made through the API: its identity as author, GitHub or itself as committer. */
function isDependabotCommit(commit) {
  if (commit.name !== DEPENDABOT_NAME || commit.email.toLowerCase() !== DEPENDABOT_EMAIL) return false;
  const committer = commit.committerEmail.toLowerCase();
  return (
    (commit.committerName === WEB_MERGE_NAME && committer === WEB_MERGE_EMAIL) ||
    (commit.committerName === DEPENDABOT_NAME && committer === DEPENDABOT_EMAIL)
  );
}

/**
 * Author identity of one commit.
 * @param {ReturnType<typeof parseGitLog>[number]} commit
 * @param {boolean} externalAuthor an outside contributor's own address is accepted
 * @returns {string[]}
 */
function checkAuthor(commit, externalAuthor) {
  const where = `commit ${commit.sha.slice(0, 8)}`;
  const who = `"${commit.name} <${commit.email}>"`;
  const author = commit.email.toLowerCase();
  if (PROJECT_EMAILS.includes(author)) {
    return commit.name === MAINTAINER_NAME
      ? []
      : [`${where}: author ${who} uses a project address under a name other than ${MAINTAINER_NAME}`];
  }
  if (author === MAINTAINER_GITHUB_NOREPLY) {
    return commit.committerEmail.toLowerCase() === WEB_MERGE_EMAIL
      ? []
      : [
          `${where}: author ${who} is the GitHub noreply address, which only GitHub's web merge may record` +
            ` (commit locally as ${MAINTAINER_NAME} <noreply@synchain.ca>)`,
        ];
  }
  if (externalAuthor) return [];
  return [
    isMaintainerName(commit.name)
      ? `${where}: ${MAINTAINER_NAME} must commit as a project address, not ${commit.email}`
      : `${where}: author ${who} is not a project address (commit as ${MAINTAINER_NAME} <noreply@synchain.ca>)`,
  ];
}

/**
 * Committer identity of one commit.
 * @param {ReturnType<typeof parseGitLog>[number]} commit
 * @param {boolean} externalAuthor
 * @returns {string[]}
 */
function checkCommitter(commit, externalAuthor) {
  const where = `commit ${commit.sha.slice(0, 8)}`;
  const who = `"${commit.committerName} <${commit.committerEmail}>"`;
  const committer = commit.committerEmail.toLowerCase();
  if (PROJECT_EMAILS.includes(committer)) {
    return commit.committerName === MAINTAINER_NAME
      ? []
      : [`${where}: committer ${who} uses a project address under a name other than ${MAINTAINER_NAME}`];
  }
  if (committer === WEB_MERGE_EMAIL) {
    return commit.committerName === WEB_MERGE_NAME
      ? []
      : [`${where}: committer ${who} uses GitHub's web-merge address under another name`];
  }
  if (committer === MAINTAINER_GITHUB_NOREPLY) {
    return [`${where}: committer ${who} is the GitHub noreply address, which no commit may use as committer`];
  }
  if (externalAuthor && committer === commit.email.toLowerCase()) return [];
  return [`${where}: committer ${who} is not a project address`];
}

/**
 * @param {{ allowExternalAuthors?: boolean, allowDependabot?: boolean }} [options]
 * @returns {string[]}
 */
function checkCommits(commits, { allowExternalAuthors = false, allowDependabot = false } = {}) {
  const patterns = loadPatterns(path.join(ROOT, PATTERNS_FILE));
  const problems = [];
  for (const commit of commits) {
    if (allowDependabot && isDependabotCommit(commit)) continue;
    const short = commit.sha.slice(0, 8);
    const isMerge = commit.parents.length > 1;
    const projectAuthor = PROJECT_EMAILS.includes(commit.email.toLowerCase());
    const webMergeAuthor = commit.email.toLowerCase() === MAINTAINER_GITHUB_NOREPLY;
    const externalAuthor = !projectAuthor && allowExternalAuthors && !isMaintainerName(commit.name);

    // A merge commit has no sign-off of its own (GitHub's web merge writes none); everything
    // else carries the DCO trailer of its author, or of the maintainer for a project commit.
    // A squash merge made on GitHub is authored by the maintainer's GitHub noreply address but
    // carries the maintainer's project sign-off from the merge message.
    const result = checkText(commit.body, {
      patterns,
      requireSignoff: !isMerge,
      signoffEmails: isMerge
        ? null
        : projectAuthor || webMergeAuthor
          ? [...PROJECT_EMAILS]
          : [commit.email],
      source: `commit ${short}`,
    });
    problems.push(...result.violations.map((v) => `commit ${short}: [${v.rule}] ${v.text}`));
    problems.push(...checkAuthor(commit, externalAuthor), ...checkCommitter(commit, externalAuthor));
    problems.push(...scanPrivacy(commit.body, `commit ${short} message`, { allowEmails: externalAuthor ? [commit.email.toLowerCase()] : [] }));
  }
  return problems;
}

/**
 * Which commits the check covers, and how strictly.
 * @returns {{ base: string, allowExternalAuthors: boolean, allowDependabot: boolean, reason: string }}
 */
function commitScope() {
  const event = process.env.EVENT_NAME || 'pull_request';
  const baseRef = env('BASE_REF', { required: false });
  const repo = env('REPO');
  const headRepo = env('HEAD_REPO', { required: false });
  const headRef = env('HEAD_REF', { required: false });
  const prAuthor = env('PR_AUTHOR', { required: false });
  const defaultBranch = env('DEFAULT_BRANCH', { required: false }) || 'dev';

  if (event === 'push') {
    const before = env('BEFORE', { required: false });
    const creates = !before || /^0+$/.test(before);
    // A feature/* branch that this push creates or rewrites was not gated by any pull request,
    // so its commits are held to project identities only.
    const strict = /^feature\//.test(env('REF_NAME', { required: false })) && creates;
    const allowExternalAuthors = !strict;
    const allowDependabot = !strict;
    if (!creates) {
      if (!/^[0-9a-f]{40}$/.test(before)) {
        throw new Error(`push "before" is not a commit SHA: ${JSON.stringify(before)}`);
      }
      if (commitExists(before)) {
        return {
          base: before,
          allowExternalAuthors,
          allowDependabot,
          reason: 'push: the commits this push added',
        };
      }
    }
    return {
      base: `origin/${defaultBranch}`,
      allowExternalAuthors,
      allowDependabot,
      reason: creates
        ? `push that creates the branch: commits not on ${defaultBranch}`
        : `push that rewrote the branch: commits not on ${defaultBranch}`,
    };
  }
  if (event === 'workflow_dispatch') {
    return {
      base: `origin/${defaultBranch}`,
      allowExternalAuthors: false,
      allowDependabot: false,
      reason: `manual run: commits not on ${defaultBranch}`,
    };
  }
  if (event === 'pull_request') {
    if (!baseRef) throw new Error('pull request without a base branch');
    const base = `origin/${baseRef}`;
    if (baseRef === 'prod') {
      return { base, allowExternalAuthors: true, allowDependabot: true, reason: "pull request into prod: dev's history" };
    }
    if (headRepo !== repo) {
      return {
        base,
        allowExternalAuthors: true,
        allowDependabot: false,
        reason: `pull request from a fork (${headRepo || 'deleted repository'})`,
      };
    }
    const fromDependabot = prAuthor === DEPENDABOT_NAME && /^dependabot\//.test(headRef ?? '');
    return {
      base,
      allowExternalAuthors: false,
      allowDependabot: fromDependabot,
      reason: fromDependabot
        ? "Dependabot's pull request: its own commits are exempt"
        : 'pull request from a branch of this repository: project identities only',
    };
  }
  throw new Error(`unsupported event ${JSON.stringify(event)}`);
}

/**
 * Source branch rules for a pull request into dev or prod.
 * @returns {string[]}
 */
function checkBranchRule({ baseRef, headRef, headRepo, repo }) {
  const fork = headRepo !== repo;
  if (baseRef === 'prod') {
    return !fork && headRef === 'dev'
      ? []
      : [`only dev of ${repo} may be merged into prod (found ${headRepo || '(deleted repository)'}:${headRef})`];
  }
  if (fork) {
    return /^(?:dev|prod|feature\/.*)$/.test(headRef)
      ? [`a pull request from a fork must not use the branch name "${headRef}" (reserved for long-lived branches)`]
      : [];
  }
  if (baseRef === 'dev' && !/^(?:feat|feature|dependabot)\/./.test(headRef)) {
    return [`only feat/*, feature/* or dependabot/* branches may be merged into dev (found "${headRef}")`];
  }
  return [];
}

// ---------------------------------------------------------------------------------------------
// Reporting

function report(title, problems, passNote) {
  if (problems.length === 0) {
    log(`${title}: PASS — ${passNote}`);
    return 0;
  }
  log(`${title}: FAIL`);
  for (const p of problems) log(`  - ${p}`);
  return 1;
}

const CHECKS = {
  scope() {
    const scope = commitScope();
    log(`commit range ${scope.base}..HEAD — ${scope.reason}`);
    process.stdout.write(
      `base=${scope.base}\nallow_external_authors=${String(scope.allowExternalAuthors)}\n` +
        `allow_dependabot=${String(scope.allowDependabot)}\n`,
    );
    return 0;
  },

  branch() {
    const pr = {
      baseRef: env('BASE_REF'),
      headRef: env('HEAD_REF'),
      headRepo: env('HEAD_REPO', { required: false }),
      repo: env('REPO'),
    };
    return report(
      'source branch',
      checkBranchRule(pr),
      `${pr.headRepo || '(deleted repository)'}:${pr.headRef} -> ${pr.baseRef}`,
    );
  },

  commits() {
    const scope = commitScope();
    log(`commit range ${scope.base}..HEAD — ${scope.reason}`);
    const commits = parseGitLog(git(['log', `--format=${GIT_LOG_FORMAT}`, `${scope.base}..HEAD`]));
    return report(
      'commits',
      checkCommits(commits, {
        allowExternalAuthors: scope.allowExternalAuthors,
        allowDependabot: scope.allowDependabot,
      }),
      `${commits.length} commit(s): signed off (merge commits exempt), no AI attribution or personal data, ` +
        'project identities',
    );
  },

  files() {
    const patterns = loadPatterns(path.join(ROOT, PATTERNS_FILE));
    const { files, problems } = textFiles();
    const scanned = files.filter((file) => file.path !== PATTERNS_FILE);
    for (const file of scanned) {
      for (const violation of checkText(file.text, { patterns, source: file.path }).violations) {
        problems.push(`${file.path}:${violation.line}: [${violation.rule}] ${violation.text}`);
      }
    }
    return report('attribution', problems, `${scanned.length} text files: no AI attribution`);
  },

  privacy() {
    const { files, problems } = textFiles();
    for (const file of files) problems.push(...scanPrivacy(file.text, file.path));
    return report(
      'privacy',
      problems,
      `${files.length} text files: no personal paths; e-mail addresses limited to ${ALLOWED_EMAILS.length} project addresses`,
    );
  },

  assets() {
    const files = repoFiles();
    const problems = files.flatMap((rel) => {
      const kind = forbiddenKind(rel, head(rel));
      return kind ? [`${rel}: ${kind}; audio and model files are never committed`] : [];
    });
    return report('assets', problems, `${files.length} files: no audio or model files`);
  },

  'pull-request-target'() {
    const dir = path.join(ROOT, '.github', 'workflows');
    // A whole-line test after the YAML comment is removed: the name as a bare or quoted token
    // that ends a key (`pull_request_target:`), a scalar, a sequence item or a flow-sequence
    // element (`on: [push, pull_request_target]`). Prose that merely mentions the name (a review
    // prompt that forbids it) is followed by other text and does not match.
    const re = /(^|[\s[,{'"])pull_request_target(['"]?\s*([:,\]}]|$))/;
    const problems = [];
    let scanned = 0;
    for (const name of fs.readdirSync(dir).filter((n) => /\.ya?ml$/.test(n))) {
      const text = fs.readFileSync(path.join(dir, name), 'utf8');
      scanned += 1;
      text.split('\n').forEach((line, i) => {
        const code = line.replace(/(^|\s)#.*$/, '');
        if (re.test(code)) problems.push(`.github/workflows/${name}:${i + 1}: ${line.trim()}`);
      });
    }
    return report('pull_request_target', problems, `${scanned} workflow file(s): no pull_request_target`);
  },

  pins() {
    // The checker and its pattern list are byte-identical copies of the maintainer's canonical
    // files; the pinned digests detect any local edit.
    const pinnedFile = path.join(ROOT, 'scripts', 'attribution-patterns.sha256');
    const pinned = new Map();
    for (const raw of fs.readFileSync(pinnedFile, 'utf8').split(/\r?\n/)) {
      const line = raw.trim();
      if (!line || line.startsWith('#')) continue;
      const m = /^([0-9a-f]{64})\s+\*?(.+)$/.exec(line);
      if (!m) throw new Error(`malformed sha256 line in scripts/attribution-patterns.sha256: ${raw}`);
      pinned.set(m[2].trim(), m[1]);
    }
    const problems = [];
    let checked = 0;
    for (const rel of ['scripts/check-attribution.mjs', PATTERNS_FILE]) {
      const expected = pinned.get(rel);
      if (!expected) {
        problems.push(`scripts/attribution-patterns.sha256 has no digest for ${rel}`);
        continue;
      }
      checked += 1;
      const actual = createHash('sha256').update(fs.readFileSync(path.join(ROOT, rel))).digest('hex');
      if (actual !== expected) problems.push(`${rel}: sha256 ${actual} != pinned ${expected}`);
    }
    return report('pins', problems, `${checked} file(s) match their pinned sha256`);
  },
};

const name = process.argv[2] ?? '';
const check = Object.hasOwn(CHECKS, name) ? CHECKS[name] : undefined;
if (!check || process.argv.length > 3) {
  log(`usage: node scripts/ci-checks.mjs <${Object.keys(CHECKS).join(' | ')}>`);
  process.exitCode = 2;
} else {
  try {
    process.exitCode = check();
  } catch (error) {
    log(`ci-checks ${name}: ${error instanceof Error ? error.message : String(error)}`);
    process.exitCode = 2;
  }
}
