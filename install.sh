#!/bin/sh
# Specify CLI installer.
# Usage:
#   curl -sSf https://specify.sh/install.sh | sh
#   curl -sSf https://specify.sh/install.sh | SPECIFY_VERSION=v0.1.0 sh
#
# Override install location:
#   SPECIFY_INSTALL_DIR=/usr/local/bin sh install.sh
#
# Skip SHA256 verification (NOT recommended):
#   SPECIFY_SKIP_VERIFY=1 sh install.sh

set -eu

REPO="augentic/specify"
BINARY="specify"

log() {
    printf '%s\n' "specify-install: $*"
}

err() {
    printf '%s\n' "specify-install: error: $*" >&2
    exit 1
}

need_cmd() {
    if ! command -v "$1" >/dev/null 2>&1; then
        err "required command not found: $1"
    fi
}

detect_os() {
    case "$(uname -s)" in
        Linux) echo "unknown-linux-gnu" ;;
        Darwin) echo "apple-darwin" ;;
        *) err "unsupported OS: $(uname -s) (supported: Linux, Darwin)" ;;
    esac
}

detect_arch() {
    case "$(uname -m)" in
        x86_64 | amd64) echo "x86_64" ;;
        arm64 | aarch64) echo "aarch64" ;;
        *) err "unsupported architecture: $(uname -m) (supported: x86_64, aarch64)" ;;
    esac
}

resolve_version() {
    if [ -n "${SPECIFY_VERSION:-}" ]; then
        echo "$SPECIFY_VERSION"
        return
    fi
    # Use the /releases/latest redirect so we don't need jq to parse JSON.
    need_cmd curl
    _url=$(curl -sSfI "https://github.com/${REPO}/releases/latest" \
        | awk 'tolower($1)=="location:" {print $2}' \
        | tr -d '\r')
    if [ -z "$_url" ]; then
        err "failed to resolve latest release (set SPECIFY_VERSION=vX.Y.Z to override)"
    fi
    echo "${_url##*/}"
}

compute_sha256() {
    _file="$1"
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$_file" | awk '{print $1}'
    elif command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$_file" | awk '{print $1}'
    else
        echo ""
    fi
}

verify_sha256() {
    _archive="$1"
    _sha_file="$2"

    if [ "${SPECIFY_SKIP_VERIFY:-0}" = "1" ]; then
        log "WARNING: SPECIFY_SKIP_VERIFY=1 set, skipping SHA256 verification"
        return 0
    fi

    if [ ! -f "$_sha_file" ]; then
        log "WARNING: no .sha256 companion found, skipping verification"
        log "         (set SPECIFY_SKIP_VERIFY=1 to suppress this warning)"
        return 0
    fi

    _expected=$(awk '{print $1}' "$_sha_file")
    _actual=$(compute_sha256 "$_archive")

    if [ -z "$_actual" ]; then
        log "WARNING: no sha256sum/shasum found, skipping verification"
        return 0
    fi

    if [ "$_expected" != "$_actual" ]; then
        err "SHA256 mismatch: expected $_expected, got $_actual"
    fi
    log "sha256 verified"
}

main() {
    need_cmd curl
    need_cmd tar
    need_cmd mktemp

    _os=$(detect_os)
    _arch=$(detect_arch)
    _target="${_arch}-${_os}"
    _version=$(resolve_version)

    case "$_version" in
        v*) ;;
        *) _version="v${_version}" ;;
    esac

    _install_dir="${SPECIFY_INSTALL_DIR:-${HOME}/.local/bin}"
    _archive="${BINARY}-${_version}-${_target}.tar.gz"
    _url="https://github.com/${REPO}/releases/download/${_version}/${_archive}"
    _sha_url="${_url}.sha256"

    log "target:  ${_target}"
    log "version: ${_version}"
    log "source:  ${_url}"
    log "dest:    ${_install_dir}/${BINARY}"

    _tmp=$(mktemp -d 2>/dev/null || mktemp -d -t specify-install)
    trap 'rm -rf "$_tmp"' EXIT INT TERM

    log "downloading archive"
    if ! curl -sSfL -o "${_tmp}/${_archive}" "$_url"; then
        err "download failed: $_url"
    fi

    log "downloading sha256"
    curl -sSfL -o "${_tmp}/${_archive}.sha256" "$_sha_url" || true

    verify_sha256 "${_tmp}/${_archive}" "${_tmp}/${_archive}.sha256"

    log "extracting"
    tar -xzf "${_tmp}/${_archive}" -C "$_tmp"

    if [ ! -f "${_tmp}/${BINARY}" ]; then
        err "archive did not contain expected binary: ${BINARY}"
    fi

    mkdir -p "$_install_dir"
    mv "${_tmp}/${BINARY}" "${_install_dir}/${BINARY}"
    chmod +x "${_install_dir}/${BINARY}"

    log "installed ${BINARY} ${_version} to ${_install_dir}/${BINARY}"

    case ":${PATH}:" in
        *":${_install_dir}:"*)
            log "run '${BINARY} --version' to verify"
            ;;
        *)
            log "NOTE: ${_install_dir} is not on your PATH."
            log "      add this line to your shell profile:"
            log "        export PATH=\"${_install_dir}:\$PATH\""
            ;;
    esac
}

main "$@"
