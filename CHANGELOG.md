# Changelog

All notable changes to `specify-cli` are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project
uses [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.3.0] - Unreleased

### Breaking changes — `schema → capability` rename

The on-disk and CLI vocabulary is unified under `capability`. The legacy
`schema:` field, `--schema` / `--proposed-schema` flags, and
`schema-name` JSON key were half-renamed in earlier releases; this
release completes the cut. There is no transitional alias.

- `JSON_SCHEMA_VERSION` bumped from `3` to `4`. Skills that pin a
  schema version must update.
- CLI flags renamed:
  - `specify slice create --schema X` → `--capability X`
  - `specify registry add --schema X` → `--capability X`
  - `specify change plan add --schema X` → `--capability X`
  - `specify change plan amend --schema X` → `--capability X`
  - `specify slice outcome set --proposed-schema X` →
    `--proposed-capability X`
- On-disk YAML keys renamed:
  - `registry.yaml: projects[].schema:` → `capability:`
  - `plan.yaml: entries[].schema:` → `capability:`
  - `<slice>/.metadata.yaml: schema:` → `capability:`
- JSON output keys renamed:
  - `init` body — `schema-name` → `capability-name`
  - `slice create` body — `schema` → `capability`
  - `registry add` body — `added.schema` → `added.capability`
  - `slice outcome set …registry-amendment-required` —
    `proposed-schema` → `proposed-capability`
- Error variant renamed: `Error::SchemaResolution(String)` →
  `Error::CapabilityResolution(String)`. The kebab-case discriminant
  emitted under `error:` becomes `capability-resolution`.

### Migration

Run the hidden one-shot migrator from the project root:

```text
specify migrate capability-noun           # rewrite in place
specify migrate capability-noun --dry-run # preview only
```

The migrator is idempotent and walks `registry.yaml`, `plan.yaml`, and
archived `.specify/archive/plans/plan-*.yaml`. It will be removed in
the next minor release.

### Other changes

- New workspace lints in `Cargo.toml` (`dbg_macro`, `todo`,
  `needless_pass_by_value`, `str_to_string`, `inefficient_to_string`,
  `items_after_statements`).
- New `cargo make standards-check` target wired into `cargo make ci`.
- Expanded coding standards in [`AGENTS.md`](AGENTS.md).

## [0.2.0]

Preceding releases are not catalogued here — see git history.
