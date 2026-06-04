# RFC-31 Phase 2 spike record

Status: **complete** (engine extensions landed; predicate retirement completed in Phase 3–4).

Parent: [RFC-31 — Declarative lint completion](https://github.com/augentic/specify/blob/main/rfcs/done/rfc-31-declarative-lints.md) (Accepted, implemented). Phase 1: [`rfc-31-phase1-spike.md`](./rfc-31-phase1-spike.md).

## Landed (Phase 2a)

- **Path-pattern exclusions:** `value` prefixed with `!` subtracts paths from the include union ([`path_pattern.rs`](../crates/standards/src/lint/eval/path_pattern.rs), [`build_candidate_set`](../crates/standards/src/lint/eval.rs)). Only-exclude rules start from the full `WorkspaceModel.files` set.
- Schema: `relativePathPattern` allows optional leading `!` in [`schemas/rules/rule.schema.json`](../schemas/rules/rule.schema.json) and resolved mirror.
- Parity: `core_025::core_025_exclusion_parity` (narrow fixture; full `OperationalVocabulary` scan roots remain imperative until Phase 3).

### Authoring example (exclusions)

```yaml
rule_hints:
  - kind: path-pattern
    value: "docs/**/*.md"
  - kind: path-pattern
    value: "!docs/explanation/decision-log.md"
  - kind: path-pattern
    value: "!**/fixtures/**"
  - kind: regex
    value: "\\bspecify validate\\b"
```

## Landed (Phase 2b)

- **`fenced_blocks`** on `WorkspaceModel` + [`extract_fenced_blocks`](../crates/standards/src/lint/index/markdown.rs).
- **`kind: fenced-block`** with source `skill-envelope-json-in-body` ([`eval/fenced_block.rs`](../crates/standards/src/lint/eval/fenced_block.rs)).
- Parity: `core_037::core_037_envelope_parity`.

## Landed (Phase 2c)

- **De-fuse pilot:** [`findings_missing_frontmatter` / `findings_schema_violation`](../crates/standards/src/framework/check/skill_frontmatter.rs) split `check_schema` emission paths (imperative `FrontmatterSchema` unchanged until Phase 3).
- **Sidecar design:** [`rfc-31-sidecar-schemas.md`](./rfc-31-sidecar-schemas.md).
- **CORE-050 production binding:** Option B (dual include globs) documented in sidecar doc; W1+candidate-set fact deferred unless glob parity fails in Phase 3.

## Completed (Phase 3–4)

- Imperative retirement per [RFC inventory](https://github.com/augentic/specify/blob/main/rfcs/done/rfc-31-declarative-lints.md#migration-inventory): 52 `CORE-*` rule files; `CORE_ID_TABLE` is CORE-009-only; `AuthoringProducer` runs namespace bridge only.
