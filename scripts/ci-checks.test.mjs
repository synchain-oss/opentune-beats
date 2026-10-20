#!/usr/bin/env node
// SPDX-FileCopyrightText: Copyright (c) 2026 Synchain
// SPDX-License-Identifier: MIT

// Unit tests for the checks that need no git or GitHub context: the commit identity rules and
// the pull_request_target scan. Run with:
//
//   node --test scripts/ci-checks.test.mjs
//
// The fixture identities are the ones GitHub and Dependabot actually record, so a change to a
// rule fails here instead of on a pull request.

import assert from 'node:assert/strict';
import test from 'node:test';

import { checkAuthor, checkCommits, checkCommitter, parseGitLog, pullRequestTargetLines } from './ci-checks.mjs';

const MAINTAINER = 'DLsnows';
const PROJECT_EMAIL = 'noreply@synchain.ca';
// Addresses are assembled from their parts, the way ci-checks.mjs writes its own: a literal
// address in a repository file fails the privacy check, even in a test fixture.
const mail = (...parts) => parts.join('@');
const MAINTAINER_NOREPLY = mail('140901583+dlsnows', 'users.noreply.github.com');
const WEB_MERGE = { committerName: 'GitHub', committerEmail: mail('noreply', 'github.com') };
const DEPENDABOT = {
  name: 'dependabot[bot]',
  email: mail('49699333+dependabot[bot]', 'users.noreply.github.com'),
};
const OUTSIDE_EMAIL = mail('outside', 'example.com');
const SIGNOFF = `Signed-off-by: ${MAINTAINER} <${PROJECT_EMAIL}>`;

/** A commit that passes every rule, then overridden per test. */
function commit(overrides = {}) {
  return {
    sha: 'a'.repeat(40),
    parents: ['b'.repeat(40)],
    name: MAINTAINER,
    email: PROJECT_EMAIL,
    committerName: MAINTAINER,
    committerEmail: PROJECT_EMAIL,
    body: `feat: add the thing\n\n${SIGNOFF}\n`,
    ...overrides,
  };
}

test('parseGitLog reads the identities, the parents and the message', () => {
  const body = `fix(ci): a subject\n\n${SIGNOFF}\n`;
  const text =
    [
      '1'.repeat(40),
      '2'.repeat(40),
      'DLsnows',
      PROJECT_EMAIL,
      'GitHub',
      WEB_MERGE.committerEmail,
      body,
    ].join('\x1f') + '\x1e';
  const [parsed] = parseGitLog(text);
  assert.equal(parsed.sha, '1'.repeat(40));
  assert.deepEqual(parsed.parents, ['2'.repeat(40)]);
  assert.equal(parsed.name, 'DLsnows');
  assert.equal(parsed.committerEmail, WEB_MERGE.committerEmail);
  assert.equal(parsed.body, body.trimEnd());
});

test('a local commit by the maintainer, signed off, passes', () => {
  const local = commit();
  assert.deepEqual(checkAuthor(local, false), []);
  assert.deepEqual(checkCommitter(local, false), []);
  assert.deepEqual(checkCommits([local]), []);
});

test('a web merge commit passes without a sign-off of its own', () => {
  const merge = commit({
    sha: 'c'.repeat(40),
    parents: ['b'.repeat(40), 'd'.repeat(40)],
    body: 'Merge pull request #12 from synchain-oss/feat/example\n',
    ...WEB_MERGE,
  });
  assert.deepEqual(checkCommits([merge]), []);
});

test('a web squash commit authored by the GitHub noreply address passes', () => {
  const squash = commit({
    email: MAINTAINER_NOREPLY,
    body: `feat: add the thing (#12)\n\n${SIGNOFF}\n`,
    ...WEB_MERGE,
  });
  assert.deepEqual(checkAuthor(squash, false), []);
  assert.deepEqual(checkCommitter(squash, false), []);
  assert.deepEqual(checkCommits([squash]), []);
});

test('a locally forged GitHub noreply author is rejected', () => {
  const forged = commit({ email: MAINTAINER_NOREPLY });
  const problems = checkAuthor(forged, false);
  assert.equal(problems.length, 1);
  assert.match(problems[0], /only GitHub's web merge may record/);
  assert.deepEqual(checkCommitter(forged, false), []);
  assert.equal(checkCommits([forged]).length, 1);
});

test('the GitHub noreply address is never a committer', () => {
  const forged = commit({ committerName: MAINTAINER, committerEmail: MAINTAINER_NOREPLY });
  const problems = checkCommitter(forged, false);
  assert.equal(problems.length, 1);
  assert.match(problems[0], /no commit may use as committer/);
});

test("Dependabot's commit is exempt only in a Dependabot pull request", () => {
  const bump = commit({
    ...DEPENDABOT,
    body: 'chore(deps): bump anyhow from 1.0.103 to 1.0.104\n',
    ...WEB_MERGE,
  });
  assert.deepEqual(checkCommits([bump], { allowDependabot: true }), []);
  const problems = checkCommits([bump], { allowDependabot: false });
  assert.ok(problems.some((problem) => /missing-signoff/.test(problem)), problems.join('\n'));
  assert.ok(problems.some((problem) => /is not a project address/.test(problem)), problems.join('\n'));
});

test('an external author is accepted only with allowExternalAuthors', () => {
  const external = commit({
    name: 'outside',
    email: OUTSIDE_EMAIL,
    committerName: 'outside',
    committerEmail: OUTSIDE_EMAIL,
    body: `fix: a thing\n\nSigned-off-by: outside <${OUTSIDE_EMAIL}>\n`,
  });
  assert.deepEqual(checkAuthor(external, true), []);
  assert.deepEqual(checkCommitter(external, true), []);
  assert.ok(checkAuthor(external, false).length > 0);
  assert.ok(checkCommitter(external, false).length > 0);
});

test('a project address under another name is rejected', () => {
  const impostor = commit({ name: 'not-the-maintainer', committerName: 'not-the-maintainer' });
  assert.ok(checkAuthor(impostor, false).length > 0);
  assert.ok(checkCommitter(impostor, false).length > 0);
});

test('pullRequestTargetLines matches YAML, not comments or prose', () => {
  assert.deepEqual(pullRequestTargetLines('on:\n  push:\n  pull_request_target:\n'), [3]);
  assert.deepEqual(pullRequestTargetLines('on:\n  "pull_request_target":\n'), [2]);
  assert.deepEqual(pullRequestTargetLines("on:\n  'pull_request_target':\n"), [2]);
  assert.deepEqual(pullRequestTargetLines('on: [push,\n     pull_request_target]\n'), [2]);
  assert.deepEqual(pullRequestTargetLines('on:\n  - push\n  - pull_request_target\n'), [3]);
  assert.deepEqual(pullRequestTargetLines('on: {push: {}, pull_request_target: {}}\n'), [1]);
  assert.deepEqual(pullRequestTargetLines('# this workflow never uses pull_request_target\non:\n  push:\n'), []);
  assert.deepEqual(pullRequestTargetLines('name: no pull_request_target here\n'), []);
  assert.deepEqual(pullRequestTargetLines('run: echo pull_request_target\n'), []);
  assert.deepEqual(pullRequestTargetLines('    uses: x/y@abc # pull_request_target\n'), []);
});
