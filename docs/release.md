# Release process

Specify publishes binaries, crates, and a Homebrew formula on every tagged release. This page describes the end-to-end flow so a maintainer can cut a release without reading workflow YAML.

## Triggering a release

Releases are driven by an annotated tag matching `v*.*.*`:

```bash
git tag -a v0.1.0 -m "Specify v0.1.0"
git push origin v0.1.0
```

The `.github/workflows/release.yml` workflow fires on the tag push and runs three jobs in order.

## Jobs that run

1. **`build` (matrix).** Compiles a release binary for each supported target:
   - `x86_64-unknown-linux-gnu` on `ubuntu-latest` (native `cargo build`).
   - `aarch64-unknown-linux-gnu` on `ubuntu-latest` via [`cross`](https://github.com/cross-rs/cross) (portable glibc toolchain, mirrors rustup's own release workflow — avoids hand-wiring `gcc-aarch64-linux-gnu` env vars per step).
   - `x86_64-apple-darwin` on `macos-13` (native).
   - `aarch64-apple-darwin` on `macos-14` (native).
   - `x86_64-pc-windows-msvc` on `windows-latest` (native).

   Each job produces a versioned archive (`specify-${TAG}-${TARGET}.tar.gz` on unix, `.zip` on Windows) plus a companion `.sha256` file, uploaded via `actions/upload-artifact@v4`.

2. **`release`.** Waits for every matrix leg, downloads all artifacts, and creates the GitHub Release with `softprops/action-gh-release@v2`. Release notes are generated from `.github/release.yaml`.

3. **`publish-crates-io`.** Gated behind `if: github.repository == 'augentic/specify'` so forks and non-canonical clones silently skip it. Publishes crates to crates.io in dependency order:

   `specify-error` → `specify-capability` → `specify-spec` → `specify-task` → `specify-change` → `specify-merge` → `specify-validate` → `specify`

   A `sleep 30` between each publish gives the crates.io index time to propagate before the next dependent crate tries to resolve it. The job reads `secrets.CARGO_REGISTRY_TOKEN`; because the job is gated at the job-level `if:`, the workflow file remains valid even in repos where the secret does not exist (GitHub only evaluates `secrets.*` inside steps that actually execute).

## Updating the Homebrew formula

The formula at `Formula/specify.rb` carries placeholder SHA256 values for the initial commit. After each release, the four platform SHA256s need to be refreshed. The sanctioned tool is [`brew bump-formula-pr`](https://docs.brew.sh/Manpage#bump-formula-pr-options-formula), which rewrites `url`, `version`, and `sha256` in a single PR against the tap.

Recipe:

```bash
VERSION="0.2.0"
for target in \
    aarch64-apple-darwin \
    x86_64-apple-darwin \
    aarch64-unknown-linux-gnu \
    x86_64-unknown-linux-gnu; do
    curl -sSfL \
        "https://github.com/augentic/specify/releases/download/v${VERSION}/specify-v${VERSION}-${target}.tar.gz.sha256"
done
```

Then, for each target:

```bash
brew bump-formula-pr \
    --url="https://github.com/augentic/specify/releases/download/v${VERSION}/specify-v${VERSION}-aarch64-apple-darwin.tar.gz" \
    --sha256="<value from above>" \
    augentic/tap/specify
```

Once the formula lands in `homebrew-core`, the tap step disappears entirely — that's a Phase-2 move.

## Install script hosting

`install.sh` lives at the repo root and is served verbatim. Whether we front it on a `specify.sh` domain or serve it as a release asset (or both) is a Phase-2 choice: the skill-fallback prose in migrated skills already tolerates both, per [RFC-1 §CLI Distribution and Fallback](../rfcs/rfc-1-cli.md) (line 1155: *"An install script (`curl -sSf https://specify.sh/install.sh | sh` or equivalent, TBD as part of Phase 1)"*).

Until a domain is purchased, users can still run:

```bash
curl -sSfL https://raw.githubusercontent.com/augentic/specify/main/install.sh | sh
```

That URL is stable and requires no infrastructure beyond the repo itself.

## Adding a new target triple

1. Add a new entry to the `matrix.include` list in `.github/workflows/release.yml`, choosing the `runs-on` runner and whether `use_cross: true` is needed.
2. If the target needs system packages (e.g. `musl-tools` for `*-musl`), add an `apt-get install` step gated on `matrix.target == '<new triple>'`.
3. Extend `Formula/specify.rb` — add a new `on_macos`/`on_linux` branch or a new `on_<platform>` block.
4. Extend `install.sh`'s `detect_os` / `detect_arch` case statements.
5. Document the new target in this file.

## Troubleshooting

- **`cross` installation fails.** Pin to a known-good commit in the `Install cross` step.
- **crates.io publish races.** If a crate fails with "dependency not found", increase the `sleep` between publishes.
- **Formula SHA256 drift.** Always regenerate after tagging — never hand-edit. The `.sha256` companion files uploaded by `release.yml` are authoritative.
