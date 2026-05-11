# Decisions

Standing architectural decisions for the `specify` CLI. Read before
changing error layering, exit codes, atomic writes, or the YAML library.

## Error layering

`specify-error` is the dependency leaf of the workspace. It depends only
on `thiserror` and `serde-saphyr`; every other `specify-*` crate may
depend on it, and it depends on none of them. Variants that need to
carry data from a downstream crate (e.g. `Error::Validation`) take a
small projection type defined in `specify-error` (`ValidationResultSummary`)
rather than re-exporting the rich domain type, so the leaf stays
cycle-free. The cost is a lossy projection at the boundary; callers that
need full fidelity reach for the downstream crate's own type directly.

## Exit codes

The binary commits to a four-slot exit-code table. `Exit::from(&Error)`
in `src/output.rs` is the single source of truth; every dispatcher routes
its error through it. `Exit::Code(u8)` is reserved for `specify tool
run` WASI passthrough.

| Code | Name                     | When                                                                                          |
|------|--------------------------|-----------------------------------------------------------------------------------------------|
| 0    | `EXIT_SUCCESS`           | Command succeeded.                                                                            |
| 1    | `EXIT_GENERIC_FAILURE`   | Any `Error` variant not listed below (I/O, YAML, schema, merge, tool resolver/runtime, ...). |
| 2    | `EXIT_VALIDATION_FAILED` | Validation findings, `Error::Validation`, or a tool request rejected as undeclared.           |
| 3    | `EXIT_VERSION_TOO_OLD`   | `project.yaml.specify_version` is newer than `CARGO_PKG_VERSION`.                             |

## Atomic writes

Use `yaml_write` (in `crates/slice/src/atomic.rs`) for any file a
concurrent reader may observe mid-write: `plan.yaml`, `.metadata.yaml`,
`journal.yaml`, `plan.lock`, and the registry. It serialises to
`NamedTempFile::new_in(parent)` and `persist`-renames over the target so
readers either see the prior bytes or the new bytes. Plain `fs::write`
is reserved for files no other process reads concurrently with the
writer (one-shot scratch output, fixtures inside a tempdir test).

## YAML library

The workspace uses `serde-saphyr` (pinned to a `0.0.x` release) for both
deserialization and serialization. It is pure-Rust, panic-free, and
actively maintained, in contrast to `serde_yaml` (deprecated) and
`serde_yaml_ng` (community fork carrying the same debt). Saphyr omits a
`Value` DOM, so code that needs untyped YAML access deserializes into
`serde_json::Value`. Its separate deser/ser error types are wrapped
behind `specify_error::YamlError` / `YamlSerError` so the upstream crate
name does not leak through every public surface.
