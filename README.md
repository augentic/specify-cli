# Specify CLI

## Installing the CLI

The `specify` binary backs every skill in the `spec` plugin. Install via (preferred order):

```bash
cargo install https://github.com/augentic/specify-cli.git
# cargo install specify                       # any platform with a Rust toolchain
# brew install augentic/tap/specify         # macOS + Linux (primary)
```

Pin a specific version with `SPECIFY_VERSION=v0.1.0` in front of the `curl` line, or override the install location with `SPECIFY_INSTALL_DIR=/usr/local/bin`. See [docs/release.md](docs/release.md) for the release process.