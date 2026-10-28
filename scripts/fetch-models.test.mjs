#!/usr/bin/env node
// SPDX-FileCopyrightText: Copyright (c) 2026 Synchain
// SPDX-License-Identifier: MIT

// Unit tests for the parts of fetch-models.mjs that need no real network: which models a run asks
// for, how a downloaded file is written, and the download limits (against a local HTTP server on
// the loopback interface). Run with:
//
//   node --test scripts/fetch-models.test.mjs

import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import http from 'node:http';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

import { MAX_MODEL_BYTES, download, isMainModule, wantedKeys, writeFresh } from './fetch-models.mjs';

const SCRIPT = fileURLToPath(new URL('./fetch-models.mjs', import.meta.url));

/** A fresh scratch directory, removed when the test ends. */
function scratch(t) {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'ot-fetch-models-'));
  t.after(() => fs.rmSync(directory, { recursive: true, force: true }));
  return directory;
}

test('wantedKeys always includes the mel model and the model asked for', () => {
  assert.deepEqual(wantedKeys('final'), ['mel', 'final']);
  assert.deepEqual(wantedKeys('small'), ['mel', 'small']);
  assert.deepEqual(wantedKeys('mel'), ['mel']);
  assert.deepEqual(wantedKeys('all'), ['mel', 'final', 'small']);
});

test('wantedKeys rejects a name that is not an own model', () => {
  for (const name of ['large', '', 'constructor', '__proto__', 'toString', 'hasOwnProperty']) {
    assert.throws(() => wantedKeys(name), /unknown model/, `model ${JSON.stringify(name)}`);
  }
});

test('the entry guard recognizes the script itself', (t) => {
  // The plain path is the script itself.
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

test('the entry guard recognizes the script through a symbolic link', (t) => {
  // Node resolves the main module to its real path, so the guard has to resolve the entry path the
  // same way. Without that, a script started through a link would exit 0 without fetching anything.
  const root = scratch(t);
  const link = path.join(root, 'fetch-models.mjs');
  try {
    fs.symlinkSync(SCRIPT, link);
  } catch (error) {
    if (error.code === 'EPERM' || error.code === 'EACCES') {
      t.skip('this account cannot create symbolic links');
      return;
    }
    throw error;
  }
  assert.equal(isMainModule(link), true);
});

test('the script exits 1 on an inherited property name, before it creates or downloads anything', (t) => {
  const root = scratch(t);
  const target = path.join(root, 'models');
  for (const name of ['constructor', '__proto__', 'toString']) {
    const run = spawnSync(process.execPath, [SCRIPT, '--dir', target, '--model', name], {
      encoding: 'utf8',
    });
    assert.equal(run.status, 1, `model ${name}: ${run.stderr}`);
    assert.match(run.stderr, /unknown model/);
    assert.equal(fs.existsSync(target), false, `model ${name} must not create the directory`);
  }
});

test('writeFresh writes a new file', (t) => {
  const file = path.join(scratch(t), 'model.onnx.part');
  writeFresh(file, Buffer.from('abc'));
  assert.equal(fs.readFileSync(file, 'utf8'), 'abc');
});

test('writeFresh replaces a regular file left behind by an earlier run', (t) => {
  const file = path.join(scratch(t), 'model.onnx.part');
  fs.writeFileSync(file, 'old and longer than the new content');
  writeFresh(file, Buffer.from('new'));
  assert.equal(fs.readFileSync(file, 'utf8'), 'new');
});

test('writeFresh never writes through a symbolic link', (t) => {
  const root = scratch(t);
  const victim = path.join(root, 'victim.txt');
  const file = path.join(root, 'model.onnx.part');
  fs.writeFileSync(victim, 'keep me');
  try {
    fs.symlinkSync(victim, file);
  } catch (error) {
    // Creating a symbolic link needs a privilege on Windows; the CI runner for this job is Linux.
    if (error.code === 'EPERM' || error.code === 'EACCES') {
      t.skip('this account cannot create symbolic links');
      return;
    }
    throw error;
  }
  assert.throws(() => writeFresh(file, Buffer.from('payload')), /symbolic link/);
  assert.equal(fs.readFileSync(victim, 'utf8'), 'keep me');
});

/** Starts a loopback server that is stopped when the test ends; returns its URL. */
async function serve(t, handler) {
  const server = http.createServer(handler);
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  t.after(async () => {
    server.closeAllConnections();
    await new Promise((resolve) => server.close(resolve));
  });
  return `http://127.0.0.1:${server.address().port}/model.onnx`;
}

test('the model size limit leaves room above the largest pinned model', () => {
  // beat_this_final.onnx is 83,162,650 bytes, the largest model.
  assert.ok(MAX_MODEL_BYTES >= 1.25 * 83_162_650);
  assert.ok(Number.isSafeInteger(MAX_MODEL_BYTES));
});

test('download returns a response within the limits', async (t) => {
  const url = await serve(t, (_request, response) => response.end('model bytes'));
  assert.equal((await download(url)).toString('utf8'), 'model bytes');
});

test('download refuses a response announced larger than the model limit', { timeout: 10_000 }, async (t) => {
  const url = await serve(t, (_request, response) => {
    response.writeHead(200, { 'content-length': String(MAX_MODEL_BYTES + 1) });
    response.flushHeaders();
  });
  await assert.rejects(download(url), /over the limit of 134217728 bytes/);
});

test('download refuses a body past an explicit limit', { timeout: 10_000 }, async (t) => {
  const url = await serve(t, (_request, response) => response.end('x'.repeat(2000)));
  await assert.rejects(download(url, { maxBytes: 1000 }), /over the limit of 1000 bytes/);
});

test('download gives up on a server that stalls', { timeout: 10_000 }, async (t) => {
  const url = await serve(t, () => {
    // Never respond.
  });
  await assert.rejects(download(url, { timeoutMs: 200 }), /no complete response within 0.2 s/);
});
