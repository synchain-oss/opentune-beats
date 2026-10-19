# Security Policy

## Supported versions

| Version | Supported |
| --- | --- |
| latest minor (x.Y.*) | ✅ |
| older | ❌ |

Nothing has been released yet. Once releases exist, only the latest minor line receives security
fixes; please upgrade to it and reproduce there first.

## Reporting a vulnerability

**Do not open a public issue.** Use GitHub Private Vulnerability Reporting: Security → Report a
vulnerability.

Backup channel, if the GitHub channel is not available: **contact@synchain.ca**, with
`[security]` in the subject.

Reports in English or Chinese are both welcome. Please include, where you can: the version
(`opentune-beats --version`) or the commit, your OS and architecture, the input file's format,
sample rate and channel count (a short synthetic sample is more useful than the original
recording — **do not send a commercial track**), and the steps to reproduce. **Do not include
credentials** of any kind.

## Response targets

- First response: within 3 working days
- Fix or mitigation: 14 days for high severity, 30 days for medium severity
- Disclosure: a public advisory (GHSA) is published 7 days after the fix ships

## Threat model in short

The tool reads one audio file and writes JSON. It opens no socket, starts no other program and
writes no file of its own. The input is untrusted: a malformed or hostile WAV must not crash the
process, read outside the file, or make it allocate unbounded memory. The model files are
untrusted too until their SHA-256 has been checked, and the check is what makes a tampered or
truncated download a clean `MODEL_CHECKSUM` error rather than a parse of attacker-controlled
data.

## Scope

- ✅ **Parsing untrusted audio**: a crafted WAV that makes the decoder read out of bounds, panic,
  loop forever, or allocate without limit; a declared sample rate or channel count that is
  absurd; NaN or infinite samples reaching the analysis.
- ✅ **Model loading**: a model file that is used without its SHA-256 being verified, a
  verification that can be skipped by an option, or a download that writes outside the models
  directory or follows a link out of it.
- ✅ **Path handling**: the input path, `--models-dir`, or the file name in the JSON output
  exposing a directory the caller did not intend to disclose.
- ✅ **Output integrity**: anything other than one JSON object on stdout, or a log line on stdout
  that a caller parsing stdout would choke on.
- ✅ **Supply chain**: a dependency with a copyleft or otherwise unusable license, a dependency
  pulled from a git URL or an unknown registry, a workflow action that is not pinned to a commit,
  a binary tool fetched without a pinned SHA-256, or excessive workflow permissions.
- ❌ Vulnerabilities in Beat This! or in beat-this-rs themselves, outside the code we add: report
  them upstream, and to us if the issue is in how we call them.
- ❌ The accuracy of a tempo estimate: that is a quality issue, not a security one. Open a normal
  issue with the input's properties and the expected tempo map.
- ❌ Attacks that require code already running as the same OS user.
