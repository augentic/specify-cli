# Specify CLI

## Installing the CLI

The `specrun` runtime binary is required by `spec` plugin skills. The `specdev` authoring binary ships alongside it for framework checks. Install using:

```bash
cargo install --git https://github.com/augentic/specify-cli

# brew install augentic/tap/specrun         # macOS + Linux (primary)
```

## Shell completions

`specrun completions <shell>` writes a completion script to stdout for any
clap-supported shell (`bash`, `elvish`, `fish`, `powershell`, `zsh`). For
example:

```bash
specrun completions zsh > "${fpath[1]}/_specrun"   # zsh
specrun completions bash > /etc/bash_completion.d/specrun  # bash
```

The script is generated from the live clap surface, so it stays in sync
with every verb the binary exposes.
