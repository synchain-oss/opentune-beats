// SPDX-FileCopyrightText: Copyright (c) 2026 Synchain
// SPDX-License-Identifier: MIT

// The one download routine of the fetch scripts (fetch-models.mjs, fetch-tool.mjs). A download is
// bounded twice: it must finish within a time limit, and it may not carry more bytes than the
// caller allows. Without either, a stalled or hostile server could hang a CI job until its
// timeout or fill the runner's memory, because the body is held in memory before it is verified.
//
// The size limit is checked on the announced `content-length` first (so an oversized file is
// refused before a byte of it is read) and again on the bytes actually received (the header can
// be absent, as with chunked transfer, or wrong).

/** How long a whole download, from the request to the last byte, may take. */
export const DOWNLOAD_TIMEOUT_MS = 120_000;

/** A refusal because of the size limit, kept apart from a timeout when an error is reported. */
class LimitError extends Error {}

/**
 * Downloads `url` into memory.
 * @param {string} url
 * @param {object} options
 * @param {number} options.maxBytes the most bytes the body may have; required
 * @param {number} [options.timeoutMs] covers the request and the whole body
 * @param {typeof fetch} [options.fetchImpl] replaceable for a test
 * @returns {Promise<Buffer>}
 */
export async function downloadBytes(
  url,
  { maxBytes, timeoutMs = DOWNLOAD_TIMEOUT_MS, fetchImpl = fetch } = {},
) {
  if (!Number.isSafeInteger(maxBytes) || maxBytes <= 0) {
    throw new Error('downloadBytes needs a positive maxBytes');
  }
  const signal = AbortSignal.timeout(timeoutMs);
  const tooLarge = (what) => new LimitError(`${url}: ${what}, over the limit of ${maxBytes} bytes`);
  try {
    const response = await fetchImpl(url, { redirect: 'follow', signal });
    if (!response.ok) throw new Error(`${url}: HTTP ${response.status}`);
    const announced = Number(response.headers.get('content-length'));
    if (Number.isFinite(announced) && announced > maxBytes) {
      await response.body?.cancel().catch(() => {});
      throw tooLarge(`the server announces ${announced} bytes`);
    }
    const chunks = [];
    let total = 0;
    if (response.body) {
      // Leaving the loop by a throw cancels the stream, which closes the connection.
      for await (const chunk of response.body) {
        total += chunk.byteLength;
        if (total > maxBytes) throw tooLarge('the body is larger than that');
        chunks.push(chunk);
      }
    }
    return Buffer.concat(chunks, total);
  } catch (error) {
    if (error instanceof LimitError) throw error;
    if (signal.aborted) {
      throw new Error(`${url}: no complete response within ${timeoutMs / 1000} s`, { cause: error });
    }
    throw error;
  }
}
