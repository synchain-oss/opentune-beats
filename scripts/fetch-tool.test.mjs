#!/usr/bin/env node
// SPDX-FileCopyrightText: Copyright (c) 2026 Synchain
// SPDX-License-Identifier: MIT

// Tests for fetch-tool.mjs that need no real network: the entry guard, and the download limits
// against a local HTTP server on the loopback interface. Run with:
//
//   node --test scripts/fetch-tool.test.mjs

import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import http from 'node:http';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

import { MAX_TOOL_ARCHIVE_BYTES, fetchTool, isMainModule } from './fetch-tool.mjs';

const SCRIPT = fileURLToPath(new URL('./fetch-tool.mjs', import.meta.url));
const SCRIPTS_DIR = path.dirname(SCRIPT);

/** A fresh scratch directory, removed when the test ends. */
function scratch(t) {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'ot-fetch-tool-'));
  t.after(() => fs.rmSync(directory, { recursive: true, force: true }));
  return directory;
}

/**
 * Links `target` at `link`. A directory is linked as a junction, which Windows allows without a
 * privilege; elsewhere the type is ignored. Returns false (and skips the test) when this account
 * may not create the link.
 */
function link(t, target, linkPath, type) {
  try {
    fs.symlinkSync(target, linkPath, type);
    return true;
  } catch (error) {
    if (error.code === 'EPERM' || error.code === 'EACCES') {
      t.skip('this account cannot create symbolic links');
      return false;
    }
    throw error;
  }
}

test('the entry guard recognizes the script itself', (t) => {
  assert.equal(isMainModule(SCRIPT), true);
  const root = scratch(t);
  // A file that exists but is not the script: the paths resolve, so the comparison decides.
  const other = path.join(root, 'other.mjs');
  fs.writeFileSync(other, '');
  assert.equal(isMainModule(other), false);
  // A path that does not exist is not the script either, and must not throw.
  assert.equal(isMainModule(path.join(root, 'missing.mjs')), false);
  // No entry path at all is not the script. An empty string stands for it: passing `undefined`
  // would take the default parameter and read `process.argv[1]` instead.
  assert.equal(isMainModule(''), false);
});

test('the entry guard recognizes the script through a link to the file', (t) => {
  const root = scratch(t);
  const file = path.join(root, 'fetch-tool.mjs');
  if (!link(t, SCRIPT, file, 'file')) return;
  assert.equal(isMainModule(file), true);
});

test('the entry guard recognizes the script through a link to its directory', (t) => {
  const root = scratch(t);
  const directory = path.join(root, 'linked-scripts');
  if (!link(t, SCRIPTS_DIR, directory, 'junction')) return;
  assert.equal(isMainModule(path.join(directory, 'fetch-tool.mjs')), true);
});

test('the script runs when started through a linked directory', (t) => {
  // Node resolves the main module to its real path but keeps argv[1] as typed. Compared with a
  // plain path.resolve, the script started through a link would run nothing and exit 0.
  const root = scratch(t);
  const directory = path.join(root, 'linked-scripts');
  if (!link(t, SCRIPTS_DIR, directory, 'junction')) return;
  const run = spawnSync(process.execPath, [path.join(directory, 'fetch-tool.mjs')], { encoding: 'utf8' });
  assert.equal(run.status, 2, `stderr: ${run.stderr}`);
  assert.match(run.stderr, /usage: node scripts\/fetch-tool\.mjs <tool>/);
});

test('the script runs when started by its plain path', () => {
  const run = spawnSync(process.execPath, [SCRIPT], { encoding: 'utf8' });
  assert.equal(run.status, 2, `stderr: ${run.stderr}`);
  assert.match(run.stderr, /usage:/);
});

/**
 * A scratch project whose lock file pins a fake tool for linux-x64, served by `handler`.
 * @returns {Promise<{ root: string, options: object }>}
 */
async function fakeProject(t, handler, sha256 = '0'.repeat(64)) {
  const server = http.createServer(handler);
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  t.after(async () => {
    server.closeAllConnections();
    await new Promise((resolve) => server.close(resolve));
  });
  const root = scratch(t);
  fs.mkdirSync(path.join(root, 'scripts'));
  const lock = {
    fake: {
      version: '1.2.3',
      url: `http://127.0.0.1:${server.address().port}/{asset}`,
      member: 'fake{ext}',
      binary: 'fake',
      assets: {
        'linux-x64': { triple: 'linux_x64', asset: 'fake_{version}_{triple}.tar.gz', sha256 },
      },
    },
  };
  fs.writeFileSync(path.join(root, 'scripts', 'tools.lock.json'), JSON.stringify(lock));
  return { root, options: { root, platform: 'linux', arch: 'x64' } };
}

/** The path where fetchTool would leave the archive of the fake tool. */
function archivePath(root) {
  return path.join(root, '.cache', 'tools', 'fake', '1.2.3', 'linux-x64', 'fake_1.2.3_linux_x64.tar.gz');
}

test('the archive size limit leaves room above the largest pinned tool', () => {
  // gitleaks 8.30.1 for Windows x64 is 8,438,883 bytes, the largest asset in tools.lock.json.
  assert.ok(MAX_TOOL_ARCHIVE_BYTES >= 4 * 8_438_883);
  assert.ok(Number.isSafeInteger(MAX_TOOL_ARCHIVE_BYTES));
});

test('an archive announced larger than the default limit is refused and not saved', { timeout: 10_000 }, async (t) => {
  const { root, options } = await fakeProject(t, (_request, response) => {
    response.writeHead(200, { 'content-length': String(MAX_TOOL_ARCHIVE_BYTES + 1) });
    response.flushHeaders();
  });
  await assert.rejects(fetchTool('fake', options), /over the limit of 67108864 bytes/);
  assert.equal(fs.existsSync(archivePath(root)), false);
});

test('a body past an explicit limit is refused and not saved', { timeout: 10_000 }, async (t) => {
  const { root, options } = await fakeProject(t, (_request, response) => {
    response.writeHead(200);
    response.end('x'.repeat(2000));
  });
  await assert.rejects(fetchTool('fake', { ...options, maxBytes: 1000 }), /over the limit of 1000 bytes/);
  assert.equal(fs.existsSync(archivePath(root)), false);
});

test('a server that stalls is a timeout and nothing is saved', { timeout: 10_000 }, async (t) => {
  const { root, options } = await fakeProject(t, () => {
    // Never respond.
  });
  await assert.rejects(fetchTool('fake', { ...options, timeoutMs: 200 }), /no complete response within 0\.2 s/);
  assert.equal(fs.existsSync(archivePath(root)), false);
});

test('a download within the limits is still checked against the pinned sha256', { timeout: 10_000 }, async (t) => {
  const { root, options } = await fakeProject(t, (_request, response) => {
    response.writeHead(200);
    response.end('not the pinned archive');
  });
  await assert.rejects(fetchTool('fake', options), /sha256 mismatch for fake_1\.2\.3_linux_x64\.tar\.gz/);
  // A mismatching archive is deleted, never kept for the next run.
  assert.equal(fs.existsSync(archivePath(root)), false);
});
