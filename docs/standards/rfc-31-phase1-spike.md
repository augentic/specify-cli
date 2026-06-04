# RFC-31 Phase 1 spike record

Status: **complete** (engine plumbing; no predicate retirement).

Parent: [RFC-31 — Declarative lint completion](https://github.com/augentic/specify/blob/main/rfcs/done/rfc-31-declarative-lints.md) (Accepted, implemented).

## Landed

- `RuleHint.config` (`Option<serde_json::Value>`), validated by `regexHintConfig` in `schemas/rules/{rule,resolved}.schema.json`.
- Extended `kind: regex` evaluator (`crates/standards/src/lint/eval/regex/`) with optional config keys:
  - `negative-match` — emit when the pattern does not match the line.
  - `capture-group` + `capture-op` + `capture-value` — numeric threshold on a capture (`lt` | `le` | `gt` | `ge` | `eq`).
  - `suffix-must-not-start-with` — per-match filter on text after the match end.
- Parity fixtures: `crates/standards/tests/core_parity.rs` modules `core_016`, `core_050`.

Absent `config`, `regex` hints behave as before Phase 1.

## `regex` config schema (authoring)

```yaml
rule_hints:
  - kind: regex
    value: "(?i)RFC[-\\s]+(\\d+)"
    config:
      capture-group: 1
      capture-op: lt
      capture-value: 100
```

```yaml
  - kind: regex
    value: "\\bspecify-contract\\b"
    config:
      suffix-must-not-start-with: "-validate"
```

## CORE-016 / CORE-050 binding (confirmed)

| Id | Binding | Parity test | Notes |
| --- | --- | --- | --- |
| CORE-016 | **W1** (`regex` + capture threshold) | `core_016::core_016_regex_parity` | Fixture uses `RFC-5` vs `RFC 3339`; full imperative `has_specify_history_citation` semantics (e.g. `rfcs/` path tokens) remain imperative until a dedicated rule or indexer fact is needed. |
| CORE-050 | **W1** (`regex` + `suffix-must-not-start-with`) | `core_050::core_050_suffix_parity` | Candidate set in parity uses `path-pattern` `plugins/**/skills/**/SKILL.md`; imperative `active_brief_and_skill_files` also includes target `briefs/` — **W1+candidate-set (W2)** if a production rule must match briefs without broadening the glob. |

## Deferred (RFC Phase 2+)

- `path-pattern` exclusion globs (`!` prefixes) for CORE-025.
- `cardinality` / other kind `config` shapes.
- Indexer facts (fence-context, frontmatter granularity, trace-staleness).
- De-fuse pilots and imperative retirement.

## Rename step (complete)

`RuleHint` / `rule_hints` (wire: `rule-hints`) replaced the former `DeterministicHint` / `deterministic_hints` names in the same program branch.
