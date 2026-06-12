# Release process

Specify publishes platform binaries and WASI tool packages on every tagged release — those are the only release artifacts. The workspace crates are not published to crates.io (there are no external crate consumers; workspace deps are path-only by design). This page describes the end-to-end flow so a maintainer can cut a release without reading workflow YAML.

## Triggering a release

Releases are driven by an annotated tag matching `v*.*.*`:

```bash
git tag -a v0.1.0 -m "Specify v0.1.0"
git push origin v0.1.0
```

The `.github/workflows/release.yaml` workflow fires on the tag push and runs three jobs in order.

## Jobs that run

1. **`build` (matrix).** Compiles a release binary for each supported target:
   - `x86_64-unknown-linux-gnu` on `ubuntu-latest` (native `cargo build`).
   - `aarch64-unknown-linux-gnu` on `ubuntu-latest` via [`cross`](https://github.com/cross-rs/cross) (portable glibc toolchain, mirrors rustup's own release workflow — avoids hand-wiring `gcc-aarch64-linux-gnu` env vars per step).
   - `x86_64-apple-darwin` on `macos-13` (native).
   - `aarch64-apple-darwin` on `macos-14` (native).
   - `x86_64-pc-windows-msvc` on `windows-latest` (native).

   Each job produces a versioned archive (`specify-${TAG}-${TARGET}.tar.gz` on unix, `.zip` on Windows) plus a companion `.sha256` file, uploaded via `actions/upload-artifact@v4`.

2. **`wasi-tools`.** Builds first-party WASI command components for `wasm32-wasip2`, publishes them as wasm-pkg packages with `wkg`, then pulls each package back for verification:

   - `specify:contract@${VERSION}`
   - `specify:vectis@${VERSION}`

   The job logs in to GHCR with the GitHub Actions token, uses the `specify -> augentic.io` wasm-pkg namespace mapping, and verifies both the built component and the pulled package with `wasm-tools validate`. Raw first-party `.wasm` files are not attached to GitHub Releases.

3. **`release`.** Waits for every native matrix leg and the Vectis WASI tools, downloads all artifacts, and creates the GitHub Release with `softprops/action-gh-release@v2`. Release notes are generated from `.github/release.yaml`.

## WASI Tool Packages

Released first-party tool declarations use exact wasm-pkg package requests:

```text
specify:contract@${VERSION}
specify:vectis@${VERSION}
```

The release workflow publishes those package requests through `wkg`; `specify tool fetch` resolves them through the embedded first-party namespace default and Augentic registry metadata. Maintainers may use `wkg` for manual inspection, but operators only need the `specify` runtime binary.

For local development before a public release, build the component into a deterministic local directory:

```bash
scripts/build-vectis-local.sh
```

This writes:

```text
target/vectis-wasi-tools/release/vectis.wasm
target/vectis-wasi-tools/release/vectis.wasm.sha256
target/vectis-wasi-tools/release/SHA256SUMS
```

To smoke-test a adapter before release, add a project-scope object declaration in `.specify/project.yaml` that keeps the adapter's tool name and permissions but overrides `source` to a local `file://` or absolute path. Include a matching `sha256` value if you want cache verification; otherwise omit `sha256` for rapid rebuilds and run `specify tool gc` when switching bytes without changing the declaration tuple. For package-path smoke tests, publish a unique prerelease package such as `specify:vectis@${VERSION}-dev.${RUN_ID}` and point a local `tools.yaml` override at that package.

## Installing a release

Download the archive for your platform from the GitHub Release page, verify it against the companion `.sha256` file, and place the `specify` binary on your `PATH`. `specify upgrade` handles subsequent updates channel-natively.

## Adding a new target triple

1. Add a new entry to the `matrix.include` list in `.github/workflows/release.yaml`, choosing the `runs-on` runner and whether `use_cross: true` is needed.
2. If the target needs system packages (e.g. `musl-tools` for `*-musl`), add an `apt-get install` step gated on `matrix.target == '<new triple>'`.
3. Document the new target in this file.

## Troubleshooting

- **`cross` installation fails.** Pin to a known-good commit in the `Install cross` step.
- **Archive SHA256 drift.** Always regenerate after tagging — never hand-edit. The `.sha256` companion files uploaded by `release.yaml` are authoritative.
