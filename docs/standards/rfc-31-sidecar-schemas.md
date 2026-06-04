# RFC-31 sidecar schema design (CORE-035 / CORE-036 / CORE-047 vs CORE-044)

Status: **design record** (Phase 2c); implementation lands in Phase 3.

Parent: [RFC-31](https://github.com/augentic/specify/blob/main/rfcs/done/rfc-31-declarative-lints.md).

## Problem

`skill.schema-violation` (`CORE-044`) validates full `schemas/authoring/skill.schema.json` frontmatter. `skill.argument-hint-grammar` (`CORE-035`), `skill.description-grammar` (`CORE-036`), and `skill.unknown-tool` (`CORE-047`) need **per-field** findings with the same titles and counts as today's imperative predicates. A single `kind: schema` hint against the monolithic skill schema would double-emit or collapse distinct messages.

## Sidecar files (proposed)

| Facet | Path | Validates |
| --- | --- | --- |
| Argument hint | `schemas/authoring/skill-argument-hint.schema.json` | `argument-hint` token grammar |
| Description | `schemas/authoring/skill-description.schema.json` | `description` leading verb + max length |
| Allowed tools | `schemas/authoring/skill-allowed-tools.schema.json` | `allowed-tools` token whitelist shape |

Each sidecar is a **JSON Schema fragment** over the parsed frontmatter object (not the full SKILL file). Rules reference them via `kind: schema` with `value: authoring/skill-<facet>.schema.json`.

## Emission rules

1. **CORE-044** — Retains monolithic `skill.schema.json`; emits only structural violations **not** covered by a sidecar (or sidecars run first and CORE-044 excludes facet keys — pick one ordering in Phase 3 PR).
2. **CORE-035 / 036 / 047** — One declarative rule each; one finding per violation; `rule_id` = `CORE-0NN`; no shared loop with CORE-044 after de-fuse (Phase 2 pilot: [`findings_missing_frontmatter` / `findings_schema_violation`](../crates/standards/src/framework/check/skill_frontmatter.rs)).
3. **Double-emission guard** — Parity modules must assert finding counts per fixture before imperative branches delete.

## CORE-050 production binding

Phase 1 confirmed **W1** on skills-only `path-pattern`. Production retirement uses **Option B** unless parity proves otherwise: include globs `plugins/**/skills/**/SKILL.md` and `adapters/targets/**/briefs/**/*.md` plus `regex` + `suffix-must-not-start-with`. Option A (`active_brief_and_skill_paths` indexer fact) remains available if glob union is too broad.
