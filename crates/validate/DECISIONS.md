# Validate crate — design decisions

Provenance for behaviour mandated by RFCs that lives in
`crates/validate/`. Code comments paraphrase these decisions as
decisionless statements; this file is the single citation point.

## Change A — Hardcoded rule registry, classification at definition site

Source: `rfcs/archive/rfc-1a-validation.md` (registry of representative
rules, declared `Classification` per rule).

The registry in `crates/validate/src/registry/` enumerates rules per
brief id. Each `Rule` and `CrossRule` declares its classification
(`Structural` or `Semantic`) at the definition site so the runner never
pattern-matches on rule prose to decide whether it can evaluate a
check. Semantic rules carry a checker that panics; the runner skips
them and emits `Deferred`. A test enforces non-invocation.

## Change B — Top-level contract format detection by root key

Source: `rfcs/archive/rfc-12-baseline-contracts.md`
§"Top-level contracts" and §Non-goals.

`crates/validate/src/contracts/parse.rs` walks the supplied
`contracts/` directory, parses every `*.yaml` file, and keeps only
documents whose root carries `openapi:` or `asyncapi:`. Filenames and
directory layout are deliberately not signals. Standalone JSON Schema
files under `contracts/schemas/` are payload vocabulary, not top-level
contracts, and are excluded by the same root-key filter. YAML parse
errors are swallowed silently — the contracts-brief verifier owns that
diagnostic.

## Change C — Baseline-contract validation rules

Source: `rfcs/archive/rfc-12-baseline-contracts.md` §Validation.

The runner in `crates/validate/src/contracts/mod.rs` enforces three
rules:

1. `contract.version-is-semver` — `info.version` must parse as
   `SemVer` (prerelease labels included; the `semver` crate decides).
2. `contract.id-format` — when `info.x-specify-id` is present,
   matches `^[a-z][a-z0-9-]*$` and is ≤ 64 characters (rule 2 cap).
3. `contract.id-unique` — every `info.x-specify-id` value is unique
   across the walked set; on duplicates, both offending paths are
   reported.

## Change D — Validate JSON envelope shape

Source: `rfcs/archive/rfc-13-contract-validate-binary.md` §4.2a.

`serialize_contract_findings` in
`crates/validate/src/contracts/envelope.rs` emits the canonical
pretty-printed JSON envelope consumed by the standalone
`specify-contract-validate` binary. The shape is byte-compatible with
the pre-Phase-2.7 `specify contract validate --format json` envelope:
top-level keys `schema-version`, `contracts-dir`, `ok`, `findings`,
`exit-code`; per-finding keys `path`, `rule-id`, `detail`. Field order
is preserved (typed `Serialize` structs piped through
`serde_json::to_string_pretty`) so the byte sequence is deterministic
and matches the legacy envelope key-for-key. Findings paths are
rendered relative to `baseline_dir.parent()` when that prefix is
present; otherwise the raw path is used.
