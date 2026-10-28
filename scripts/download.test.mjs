#!/usr/bin/env node
// SPDX-FileCopyrightText: Copyright (c) 2026 Synchain
// SPDX-License-Identifier: MIT

// Tests for the bounded download shared by the fetch scripts. They use a local HTTP server on the
// loopback interface (no network) and an injected fetch. Run with:
//
//   node --test scripts/download.test.mjs

import assert from 'node:assert/strict';
import http from 'node:http';
import test from 'node:test';

import { DOWNLOAD_TIMEOUT_MS, downloadBytes } from './download.mjs';

/**
 * Starts a server on a free loopback port and stops it when the test ends.
 * @param {import('node:test').TestContext} t
 * @param {http.RequestListener} handler
 * @returns {Promise<string>} the base URL
 */
async function serve(t, handler) {
  const server = http.createServer(handler);
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  t.after(async () => {
    server.closeAllConnections();
    await new Promise((resolve) => server.close(resolve));
  });
  return `http://127.0.0.1:${server.address().port}/file`;
}

test('the time limit is two minutes', () => {
  assert.equal(DOWNLOAD_TIMEOUT_MS, 120_000);
});

test('a response within the limits is returned whole', async (t) => {
  const url = await serve(t, (_request, response) => {
    response.writeHead(200, { 'content-length': '5' });
    response.end('hello');
  });
  const bytes = await downloadBytes(url, { maxBytes: 100 });
  assert.equal(bytes.toString('utf8'), 'hello');
});

test('a body of exactly the limit is accepted, one byte more is not', async (t) => {
  const url = await serve(t, (request, response) => {
    // No content-length: chunked transfer, so only the received bytes can be counted.
    response.write('x'.repeat(10));
    response.end(request.url.endsWith('more') ? 'y' : '');
  });
  assert.equal((await downloadBytes(url, { maxBytes: 10 })).length, 10);
  await assert.rejects(downloadBytes(`${url}-more`, { maxBytes: 10 }), /over the limit of 10 bytes/);
});

test('the request carries a timeout signal that is not yet aborted', async () => {
  let seen = null;
  const fetchImpl = async (_url, options) => {
    seen = options.signal;
    return new Response('ok');
  };
  await downloadBytes('http://example.invalid/file', { maxBytes: 10, fetchImpl });
  assert.ok(seen instanceof AbortSignal);
  assert.equal(seen.aborted, false);
});

test('a server that never answers is a timeout, not a hang', { timeout: 10_000 }, async (t) => {
  const url = await serve(t, () => {
    // Never respond.
  });
  await assert.rejects(downloadBytes(url, { maxBytes: 100, timeoutMs: 200 }), /no complete response within 0\.2 s/);
});

test('a body that stalls half way is a timeout too', { timeout: 10_000 }, async (t) => {
  const url = await serve(t, (_request, response) => {
    response.writeHead(200);
    response.write('part of the body');
    // ...and never the rest.
  });
  await assert.rejects(downloadBytes(url, { maxBytes: 1000, timeoutMs: 300 }), /no complete response within 0\.3 s/);
});

test('an announced length over the limit is refused before the body is read', { timeout: 10_000 }, async (t) => {
  const url = await serve(t, (_request, response) => {
    // The header promises 1 MiB and the body never comes: reading it would only hit the timeout.
    response.writeHead(200, { 'content-length': String(1024 * 1024) });
    response.flushHeaders();
  });
  // The default timeout is two minutes: the rejection has to come from the header, long before.
  await assert.rejects(downloadBytes(url, { maxBytes: 1000 }), /announces 1048576 bytes, over the limit of 1000 bytes/);
});

test('a body that grows past the limit without a length is cut off', { timeout: 10_000 }, async (t) => {
  let sent = 0;
  const url = await serve(t, (_request, response) => {
    response.writeHead(200);
    const timer = setInterval(() => {
      sent += 400;
      response.write('z'.repeat(400));
    }, 5);
    response.on('close', () => clearInterval(timer));
  });
  await assert.rejects(downloadBytes(url, { maxBytes: 1000 }), /over the limit of 1000 bytes/);
  // The reader stopped: the server was not asked to deliver much more than the limit.
  assert.ok(sent < 1000 * 50, `sent ${sent} bytes`);
});

test('an HTTP error status is reported with the status', async (t) => {
  const url = await serve(t, (_request, response) => {
    response.writeHead(404);
    response.end('not here');
  });
  await assert.rejects(downloadBytes(url, { maxBytes: 100 }), /HTTP 404/);
});

test('an injected fetch that fails keeps its own error', async () => {
  const fetchImpl = async () => {
    throw new Error('connection refused');
  };
  await assert.rejects(downloadBytes('http://example.invalid/', { maxBytes: 10, fetchImpl }), /connection refused/);
});

test('a limit that is not a positive integer is refused', async () => {
  for (const maxBytes of [undefined, 0, -1, 1.5, Number.NaN]) {
    await assert.rejects(downloadBytes('http://example.invalid/', { maxBytes }), /positive maxBytes/);
  }
});
