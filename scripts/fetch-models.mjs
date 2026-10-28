#!/usr/bin/env node
// Download the ONNX models into a directory, verifying each file against the digest the binary
// was built with (src/models.rs is the source of truth for these four values).
//
//   node scripts/fetch-models.mjs [--dir models] [--model final|small|all]
//
// `mel` is always fetched, because both beat models need it. A file that already matches is
// left alone, so running this against a restored CI cache costs one hash per file.

import { createHash } from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

import { downloadBytes } from './download.mjs';

/**
 * The most bytes one model download may carry. The largest model, beat_this_final.onnx, is
 * 83,162,650 bytes (79.3 MiB); 128 MiB leaves about 50% of room for a model that is re-pinned
 * slightly larger, and still refuses anything that is clearly not a model. The digest is checked
 * after the download, so this limit only keeps a wrong or hostile response out of memory.
 */
export const MAX_MODEL_BYTES = 128 * 1024 * 1024;

const PIN = '1ae768e78f1ad83b0ed3886241dc29ffde853c40';
const RAW = `https://raw.githubusercontent.com/danigb/beat-this-rs/${PIN}/models`;
const MODELS = {
  mel: {
    file: 'mel_spectrogram.onnx',
    url: `${RAW}/mel_spectrogram.onnx`,
    sha256: 'fdd59e65c515331308e4c8841edf99972deca646bdf6197744c2a5b7755e3de9',
  },
  final: {
    file: 'beat_this_final.onnx',
    url: 'https://github.com/danigb/beat-this-rs/releases/download/model-large/beat_this.onnx',
    sha256: '5f810debe53459b559127fb55bbad40035bb47cc567b20e501670f968c770f02',
  },
  small: {
    file: 'beat_this_small.onnx',
    url: `${RAW}/beat_this_small.onnx`,
    sha256: 'a5f8d39d989f31859454ba27afe61c5317ca95e4d9373e6853e5361b8937172f',
  },
};

function parseArgs(argv) {
  const options = { dir: 'models', model: 'final' };
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (arg === '--dir' || arg === '--model') {
      const value = argv[index + 1];
      if (value === undefined) throw new Error(`missing value for ${arg}`);
      options[arg.slice(2)] = value;
      index += 1;
    } else {
      throw new Error(`unknown option ${arg}`);
    }
  }
  return options;
}

function digest(file) {
  return createHash('sha256').update(fs.readFileSync(file)).digest('hex');
}

/**
 * Downloads one model into memory, with a time limit (DOWNLOAD_TIMEOUT_MS) and the size limit
 * above. `options` is only for a test.
 * @param {string} url
 * @param {{ maxBytes?: number, timeoutMs?: number, fetchImpl?: typeof fetch }} [options]
 * @returns {Promise<Buffer>}
 */
export function download(url, { maxBytes = MAX_MODEL_BYTES, ...rest } = {}) {
  return downloadBytes(url, { maxBytes, ...rest });
}

/**
 * Writes the bytes to a fresh file. An existing path is never followed: a symbolic link there
 * (a `.part` left behind by something else) is refused, a regular file is removed first, and the
 * final open is exclusive (`wx`), so a file created between the check and the write fails the
 * run instead of being written through.
 * @param {string} file
 * @param {Buffer} bytes
 */
export function writeFresh(file, bytes) {
  let stat = null;
  try {
    stat = fs.lstatSync(file);
  } catch (error) {
    if (error.code !== 'ENOENT') throw error;
  }
  if (stat?.isSymbolicLink()) throw new Error(`${file}: refusing to write through a symbolic link`);
  if (stat) fs.rmSync(file);
  fs.writeFileSync(file, bytes, { flag: 'wx' });
}

async function ensure(directory, model) {
  const target = path.join(directory, model.file);
  if (fs.existsSync(target)) {
    if (digest(target) === model.sha256) {
      console.error(`ok        ${model.file}`);
      return;
    }
    console.error(`replacing ${model.file}: the digest does not match`);
    fs.rmSync(target);
  }
  console.error(`downloading ${model.file} from ${model.url}`);
  const bytes = await download(model.url);
  const actual = createHash('sha256').update(bytes).digest('hex');
  if (actual !== model.sha256) {
    throw new Error(`${model.file}: sha256 ${actual} != ${model.sha256}`);
  }
  const partial = `${target}.part`;
  writeFresh(partial, bytes);
  fs.renameSync(partial, target);
  console.error(`saved     ${model.file} (${bytes.length} bytes)`);
}

/**
 * The model keys a run fetches: `mel` always, plus the beat model(s) asked for. A name that only
 * exists on the prototype chain (`constructor`, `__proto__`) is not a model.
 * @param {string} model `final`, `small`, `mel` or `all`
 * @returns {string[]}
 */
export function wantedKeys(model) {
  if (model === 'all') return ['mel', 'final', 'small'];
  if (!Object.hasOwn(MODELS, model)) throw new Error(`unknown model ${model}`);
  return model === 'mel' ? ['mel'] : ['mel', model];
}

async function main() {
  try {
    const options = parseArgs(process.argv.slice(2));
    const wanted = wantedKeys(options.model);
    fs.mkdirSync(options.dir, { recursive: true });
    for (const key of wanted) {
      await ensure(options.dir, MODELS[key]);
    }
  } catch (error) {
    console.error(`fetch-models: ${error instanceof Error ? error.message : String(error)}`);
    process.exitCode = 1;
  }
}

/**
 * True when this file is the one the process was started with. Both sides are resolved through
 * symbolic links: `import.meta.url` holds the real path of the module, while `process.argv[1]`
 * holds the path as it was typed. A path that cannot be resolved is not the main module.
 * @param {string} [entry] the path the process was started with
 * @returns {boolean}
 */
export function isMainModule(entry = process.argv[1]) {
  if (!entry) return false;
  try {
    return fs.realpathSync(entry) === fileURLToPath(import.meta.url);
  } catch {
    return false;
  }
}

// Run only when started as a script, so a test can import the helpers above. The entry path is
// resolved through symbolic links before it is compared: Node holds the real path of the main
// module in `import.meta.url`, but `process.argv[1]` keeps the path as it was typed, so a script
// started through a link would otherwise never run and would exit 0 without fetching anything.
if (isMainModule()) {
  await main();
}
