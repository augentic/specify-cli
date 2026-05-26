#!/usr/bin/env sh
set -eu

# RFC-28 §"Relationship to framework authoring": the vendored runtime codex
# rule schema must match the authoring source byte-for-byte. This script is
# the only sanctioned way to refresh the vendored copy — never hand-edit it,
# and never reformat or run the bytes through `jq`.
#
# Run from the augentic/specify-cli workspace root.

SRC="crates/authoring/schemas/codex-rule.schema.json"
DST="schemas/codex/codex-rule.schema.json"

if [ ! -f "$SRC" ]; then
  echo "sync-codex-schema: $SRC not found; run from the specify-cli workspace root" >&2
  exit 2
fi

mkdir -p "$(dirname "$DST")"
cp "$SRC" "$DST"
echo "sync-codex-schema: synced $SRC -> $DST"
