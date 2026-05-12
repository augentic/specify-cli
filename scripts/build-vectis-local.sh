#!/usr/bin/env bash
# Build the `vectis` WASI tool locally and emit sha256 sidecars,
# mirroring the bytes the release workflow publishes through `wkg`.
# Use this for pre-release smoke tests via a `file://` source in a
# capability's `tools.yaml`.
#
# Output directory is controlled by `VECTIS_WASI_DIST_DIR` and
# defaults to `target/vectis-wasi-tools/release/`.
#
# Writes:
#   ${DIST_DIR}/vectis.wasm
#   ${DIST_DIR}/vectis.wasm.sha256
#   ${DIST_DIR}/SHA256SUMS
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${REPO_ROOT}"

DIST_DIR="${VECTIS_WASI_DIST_DIR:-target/vectis-wasi-tools/release}"
mkdir -p "${DIST_DIR}"
DIST_DIR_ABS="$(cd "${DIST_DIR}" && pwd)"

(
    cd wasi-tools
    cargo build -p specify-vectis --target wasm32-wasip2 --release --locked
    cp target/wasm32-wasip2/release/vectis.wasm "${DIST_DIR_ABS}/vectis.wasm"
)

(
    cd "${DIST_DIR_ABS}"
    shasum -a 256 vectis.wasm > vectis.wasm.sha256
    shasum -a 256 vectis.wasm > SHA256SUMS
)

echo "Vectis WASI artifact written to ${DIST_DIR_ABS}"
