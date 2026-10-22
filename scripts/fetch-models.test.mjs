#!/usr/bin/env node
// SPDX-FileCopyrightText: Copyright (c) 2026 Synchain
// SPDX-License-Identifier: MIT

// Unit tests for the parts of fetch-models.mjs that need no network: which models a run asks for,
// and how a downloaded file is written. Run with:
//
//   node --test scripts/fetch-models.test.mjs

import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

import { wantedKeys, writeFresh } from './fetch-models.mjs';

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
