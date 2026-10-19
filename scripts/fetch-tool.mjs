#!/usr/bin/env node
// SPDX-FileCopyrightText: Copyright (c) 2026 Synchain
// SPDX-License-Identifier: MIT

// Downloads a pinned binary tool into .cache/tools/ and verifies it before use. Version, asset
// names and sha256 digests come only from scripts/tools.lock.json; nothing here hardcodes a
// version, so bumping a tool is a one-line change to that file.
//
// A digest mismatch is fatal: the archive is deleted and nothing is extracted.
//
// Usage: node scripts/fetch-tool.mjs <tool>     prints the path of the verified binary

import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import fs from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

export const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
export const LOCK_FILE = 'scripts/tools.lock.json';
export const CACHE_DIR = '.cache/tools';

/**
 * @typedef {object} ResolvedTool
 * @property {string} name
 * @property {string} version
 * @property {string} asset    release asset (archive) file name
 * @property {string} url      download URL of the asset
 * @property {string} sha256   expected sha256 of the asset, lowercase hex
 * @property {string} member   path of the executable inside the archive
 * @property {string} binary   file name of the executable after extraction
 * @property {string} dir      cache directory for this version
 */

/**
 * Reads the pin for `name` and resolves the asset for this platform.
 * @param {string} name
 * @param {{ root?: string, platform?: string, arch?: string }} [options]
 * @returns {Promise<ResolvedTool>}
 */
export async function resolveTool(name, { root = ROOT, platform = process.platform, arch = process.arch } = {}) {
  const lock = JSON.parse(await fs.readFile(path.join(root, LOCK_FILE), 'utf8'));
  const pin = lock[name];
  if (!pin || typeof pin.version !== 'string' || !/^\d+\.\d+\.\d+$/.test(pin.version)) {
    throw new Error(`${LOCK_FILE}: no valid pin for "${name}"`);
  }
  const key = `${platform}-${arch}`;
  const entry = pin.assets?.[key];
  if (!entry) throw new Error(`${LOCK_FILE}: ${name} ${pin.version} has no asset for ${key}`);
  if (!/^[0-9a-f]{64}$/.test(entry.sha256)) throw new Error(`${LOCK_FILE}: ${name}/${key} has no valid sha256`);
  // A target triple (x86_64-pc-windows-msvc) or the release's shorter platform tag (windows_x64).
  if (typeof entry.triple !== 'string' || !/^[a-z0-9][a-z0-9_-]*$/.test(entry.triple)) {
    throw new Error(`${LOCK_FILE}: ${name}/${key} has no valid target triple`);
  }
  const fill = (template) =>
    template
      .replaceAll('{version}', pin.version)
      .replaceAll('{triple}', entry.triple)
      .replaceAll('{ext}', platform === 'win32' ? '.exe' : '');
  const asset = fill(entry.asset);
  const url = fill(pin.url).replaceAll('{asset}', asset);
  const member = fill(pin.member);
  const binary = `${pin.binary ?? name}${platform === 'win32' ? '.exe' : ''}`;
  const dir = path.join(root, CACHE_DIR, name, pin.version, key);
  return { name, version: pin.version, asset, url, sha256: entry.sha256, member, binary, dir };
}

async function sha256Of(file) {
  return createHash('sha256')
    .update(await fs.readFile(file))
    .digest('hex');
}

async function download(url, file) {
  const response = await fetch(url, { redirect: 'follow' });
  if (!response.ok) throw new Error(`download failed: ${url} -> HTTP ${response.status}`);
  await fs.writeFile(file, Buffer.from(await response.arrayBuffer()));
}

function extract(archive, dir, member) {
  // Windows ships bsdtar as System32\tar.exe, which also reads zip; resolve it explicitly so a
  // GNU tar earlier on PATH (Git Bash, MSYS) is not picked up.
  const tar =
    process.platform === 'win32'
      ? path.join(process.env.SystemRoot ?? 'C:\\Windows', 'System32', 'tar.exe')
      : 'tar';
  const args = archive.endsWith('.zip') ? ['-xf', archive, '-C', dir, member] : ['-xzf', archive, '-C', dir, member];
  // tar's own messages go straight to this process's stderr: nothing here parses them, and a
  // captured pipe would have to be drained to stay reliable.
  const result = spawnSync(tar, args, { stdio: ['ignore', 'ignore', 'inherit'] });
  if (result.error) throw new Error(`running ${tar} failed: ${result.error.message}`);
  if (result.status !== 0) {
    throw new Error(`extracting ${member} from ${archive} failed (exit ${result.status})`);
  }
}

/**
 * Downloads (or reuses) the verified archive and extracts the binary. Returns its path.
 * @param {string} name
 * @param {{ root?: string, platform?: string, arch?: string }} [options]
 * @returns {Promise<ResolvedTool & { path: string }>}
 */
export async function fetchTool(name, options = {}) {
  const tool = await resolveTool(name, options);
  await fs.mkdir(tool.dir, { recursive: true });
  const archive = path.join(tool.dir, tool.asset);
  let digest = await sha256Of(archive).catch(() => null);
  if (digest !== tool.sha256) {
    await download(tool.url, archive);
    digest = await sha256Of(archive);
  }
  if (digest !== tool.sha256) {
    await fs.rm(archive, { force: true });
    throw new Error(`sha256 mismatch for ${tool.asset}: expected ${tool.sha256}, got ${digest}`);
  }
  // Re-extract every time so the binary always comes from the archive just verified.
  const binary = path.join(tool.dir, tool.binary);
  await fs.rm(binary, { force: true });
  extract(archive, tool.dir, tool.member);
  if (tool.member !== tool.binary) {
    // The archive keeps its executable inside a versioned directory; move it next to the cache
    // root so the caller always gets the same path.
    await fs.rm(binary, { force: true });
    await fs.rename(path.join(tool.dir, tool.member), binary);
  }
  if (process.platform !== 'win32') await fs.chmod(binary, 0o755);
  return { ...tool, path: binary };
}

const isMain = process.argv[1] !== undefined && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url);
if (isMain) {
  const name = process.argv[2];
  if (!name) {
    process.stderr.write('usage: node scripts/fetch-tool.mjs <tool>\n');
    process.exitCode = 2;
  } else {
    try {
      const tool = await fetchTool(name);
      process.stdout.write(`${tool.path}\n`);
    } catch (error) {
      process.stderr.write(`fetch-tool: ${error instanceof Error ? error.message : String(error)}\n`);
      process.exitCode = 1;
    }
  }
}
