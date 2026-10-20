# Release

How a release of OpenTune Beats is built, what it ships, and how to rehearse it. The workflow is
[`.github/workflows/release.yml`](../.github/workflows/release.yml); the asset contract is section 5
of [`docs/output.md`](./output.md), and the version lives only in `Cargo.toml`.

## Assets

| Asset | Contents |
| --- | --- |
| `opentune-beats-<version>-win-x64.zip` | `opentune-beats.exe` (Windows x64, static CRT), `LICENSE`, `NOTICE`, `THIRD-PARTY-NOTICES` |
| `opentune-beats-<version>-macos-universal2.tar.gz` | `opentune-beats` (Apple silicon and Intel, merged with `lipo`), `LICENSE`, `NOTICE`, `THIRD-PARTY-NOTICES` |
| `mel_spectrogram.onnx` | The log-mel front end both beat models need |
| `beat_this_final.onnx` | `--model final`, the default |
| `beat_this_small.onnx` | `--model small` |
| `SHA256SUMS` | The digest of every asset above, in `sha256sum -c` format |

Naming rules:

- The two archives carry the version from `Cargo.toml`, in the exact form
  `opentune-beats-<version>-<target>.<ext>` with `<target>` either `win-x64` or
  `macos-universal2`. The caller (the `beat-grid` skill of OpenTune MCP Server) looks the asset up
  by this name.
- The three model files keep their upstream names and carry no version: they are the same files on
  every platform, and the binary compiles their SHA-256 in, so their names are part of the CLI
  contract (`version --json`).
- The executable is at the root of both archives, never inside a directory. The Windows zip
  contains forward-slash entry names only; the macOS archive keeps the executable bit on
  `opentune-beats`.

## Steps (maintainer)

1. **Bump the version.** Change `version` in `Cargo.toml` on `dev`, and merge `dev` into
   `prod` after review. `prod` is the only branch releases are built from. The version must be
   `x.y.z`: only final versions are released, never a pre-release (no `-rc.1` or similar suffix);
   the preflight job and `scripts/release-notes.mjs` both reject anything else.
2. **Push the tag.** Create `v<version>` at the `prod` commit and push it:
   `git tag v0.1.0 && git push origin v0.1.0`. The tag name must be `v` plus the `Cargo.toml`
   version, or the run stops in `preflight`.
3. **Let the workflow run.** A tag push starts, in order:
   - `preflight`: the tag names the `Cargo.toml` version and points at a commit reachable from
     `prod` (`git merge-base --is-ancestor`);
   - `checks`: the whole CI suite through `workflow_call` (`ci.yml`: fmt, clippy, tests,
     compliance, `cargo deny`, the notices check and the accuracy bench);
   - `build-windows`, `build-macos`, `models`: build and smoke-test both binaries, pack the two
     archives, fetch the pinned models;
   - `package`: collect the assets, write `SHA256SUMS`, generate the release body with
     `scripts/release-notes.mjs` and check it with `scripts/check-attribution.mjs`;
   - `release`: create the release as a **draft**, with all assets and that body.
4. **Review the draft.** Check the asset list (five files plus `SHA256SUMS`), the digests in the
   body, and the body text. Then promote the draft in the GitHub UI.
5. **Record the digests in the caller.** Copy the SHA-256 of the two archives and of the three
   model files into the manifest of the `beat-grid` skill. They come from `SHA256SUMS` and from the
   release body; the model digests only change when the pinned URLs in `scripts/fetch-models.mjs`
   change.

Only a maintainer publishes a release, and only through the GitHub UI: the workflow creates the
draft and stops there. Never move a published tag; publish a new patch version instead.

## Dry run

`Actions → Release → Run workflow` on any branch runs everything except the two tag checks and the
release: `preflight` reads the version, the CI suite runs, both targets build and pack, the models
are fetched and verified, `package` writes `SHA256SUMS` and the body, and the run summary lists
the digests. The result is the `release-assets` artifact of that run; no release is created.

Use it after changing the workflow, after a `Cargo.toml` version bump (a dry run on `dev` proves
the archive names), and before pushing a tag.

## What the workflow refuses

- A tag that is not `v` + the `Cargo.toml` version.
- A tag whose commit is not on `prod`.
- An asset set that is not exactly the five files above (`scripts/release-notes.mjs` fails on a
  missing or an unexpected file).
- A zip entry with a backslash, and a `tar.gz` that does not keep the executable bit.
- A model whose bytes do not match the pinned SHA-256.
- A release body that fails the attribution rules.
- Anything, when the CI suite fails: the release job needs `checks` and `package`.

## Doing it by hand

Only for debugging; the workflow is the supported path. The commands it runs are:

```bash
cargo build --release --locked --target x86_64-pc-windows-msvc       # Windows x64
cargo build --release --locked --target aarch64-apple-darwin         # macOS, both slices
cargo build --release --locked --target x86_64-apple-darwin
lipo -create -output opentune-beats \
  target/aarch64-apple-darwin/release/opentune-beats \
  target/x86_64-apple-darwin/release/opentune-beats
node scripts/fetch-models.mjs --dir models --model all
cargo fetch --locked                                                # gen-notices reads the packages
node scripts/release-notes.mjs --version 0.1.0 --assets dist
node scripts/check-attribution.mjs dist/release-notes.md
node scripts/gen-notices.mjs --check
```

`dist/` must already hold the two archives and the three model files; `release-notes.mjs` writes
`dist/SHA256SUMS` and, with `--out`, the body it would publish.
