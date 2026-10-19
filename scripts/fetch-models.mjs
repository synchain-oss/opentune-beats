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

async function download(url) {
  const response = await fetch(url, { redirect: 'follow' });
  if (!response.ok) throw new Error(`${url}: HTTP ${response.status}`);
  return Buffer.from(await response.arrayBuffer());
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
  fs.writeFileSync(partial, bytes);
  fs.renameSync(partial, target);
  console.error(`saved     ${model.file} (${bytes.length} bytes)`);
}

try {
  const options = parseArgs(process.argv.slice(2));
  const wanted = options.model === 'all' ? ['mel', 'final', 'small'] : ['mel', options.model];
  if (options.model !== 'all' && !(options.model in MODELS)) {
    throw new Error(`unknown model ${options.model}`);
  }
  fs.mkdirSync(options.dir, { recursive: true });
  for (const key of wanted) {
    await ensure(options.dir, MODELS[key]);
  }
} catch (error) {
  console.error(`fetch-models: ${error instanceof Error ? error.message : String(error)}`);
  process.exitCode = 1;
}
