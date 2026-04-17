# Specify CLI

## Installing the CLI

The `specify` binary backs every skill in the `spec` plugin. Install via (preferred order):

```bash
brew install augentic/tap/specify           # macOS + Linux (primary)
cargo install specify                       # any platform with a Rust toolchain
curl -sSfL https://specify.sh/install.sh | sh   # pre-built binary, any POSIX shell
make build                                  # local checkout, drops ./specify at repo root
```

Pin a specific version with `SPECIFY_VERSION=v0.1.0` in front of the `curl` line, or override the install location with `SPECIFY_INSTALL_DIR=/usr/local/bin`. See [docs/release.md](docs/release.md) for the release process.