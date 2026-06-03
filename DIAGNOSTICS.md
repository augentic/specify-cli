# Diagnostics lint unification — status and remaining scope

Status record for the lint unification work originally tracked as **A19** (unify lint output path + framework/consumer dispatch) and **A16** (imperative→declarative lint burn-down). The two former binaries (`specrun` runtime + `specdev` authoring lint) have since converged onto a single `specify` binary; the framework authoring lint is now `specify lint framework`.

This document was rewritten after an audit of the live tree (2026-06) found the earlier implementation plan's baseline stale: A19 is complete, the A16 "Wave 0" retirements already landed, and the remaining A16 burn-down is gated on engine work that no amount of author-side rule writing can substitute for. **[RFC-31](https://github.com/augentic/specify/blob/main/rfcs/RFC-31-declarative-lints.md)** (Accepted) is that engine program; Phases 0–1 spike status lives in [`docs/standards/rfc-31-phase1-spike.md`](./docs/standards/rfc-31-phase1-spike.md). Until RFC-31 Phase 4 completes, the imperative predicates behind `framework::check::run` remain authoritative for migratable ids not yet retired at parity.

Scope: `augentic/specify-cli` (primary) and `augentic/specify` (CORE rule files, docs).

Related docs:

- [DECISIONS.md §"Drained `Error::Validation` and the `Diagnostic` substrate"](./DECISIONS.md#drained-errorvalidation-and-the-diagnostic-substrate)
- [DECISIONS.md §"Crate layout"](./DECISIONS.md) — the framework-authoring-checks paragraph that governs the steady-state posture
- [docs/standards/handler-shape.md](./docs/standards/handler-shape.md) — "The two lint handlers share one tail"
- [adapters/shared/rules/core/README.md](https://github.com/augentic/specify/blob/main/adapters/shared/rules/core/README.md) (framework repo)
- [docs/quality-debt.md](./docs/quality-debt.md) — suppression burn-down tied to A16

---

## A19 — Unify lint output path and dispatch — COMPLETE

Both lint surfaces converge on one kernel. `specify lint run` ([src/runtime/commands/lint/run.rs](./src/runtime/commands/lint/run.rs)) and `specify lint framework` ([src/runtime/commands/lint/framework.rs](./src/runtime/commands/lint/framework.rs)) each return `Result<()>` and call `output::run_lint(format, || build_report(...))` in [src/output.rs](./src/output.rs). The kernel owns the shared tail — `emit_lint_report` runs the pipeline and renders the envelope, the internal `finish_lint` collapses the outcome into the terminal `Result<()>` (`deny_blocking_findings` on success, the empty-envelope JSON fallback on a pre-emit abort). The two handlers differ only in the `PipelineConfig` their `build_report` closure assembles.

What this closed (relative to the original A19 gap list):

- **Bespoke `Exit` enum** — gone. There is no `src/authoring/exit.rs`; the framework lint's terminal error maps through the one `Exit::from(&Error)` table in [src/runtime/output.rs](./src/runtime/output.rs).
- **Manual error / `eprintln!` paths** — gone. Neither handler writes its own `println!`/`eprintln!`; the abort fallback lives only in `output::finish_lint`.
- **Handler-shape divergence** — gone. Both handlers obey the same `Result<()>` contract documented in [handler-shape.md](./docs/standards/handler-shape.md).
- **Shared kernel** — landed. `output::run_lint` is the single build → emit → finish → blocking-gate kernel; `finish_lint` is internal to it.

Pinned wire contract (unchanged):

- `LintEmit::trailing_newline` — `true` for `specify lint`, `false` for `specify lint framework` — preserves each surface's historical stdout shape. It is caller config, not normalised, until an intentional wire bump.
- `--format json` / `--output-format json` emit a `DiagnosticReport` on stdout even on infrastructure failure (empty all-zero envelope), so CI consumers keep a stable shape.

Verification: `cargo make check`; `cargo nextest run -p specify-standards --test lint_diagnostics_json --test lint_diagnostics_pretty`; `cargo nextest run -p specify lint`.

---

## A16 — Imperative→declarative lint burn-down — BOUNDED; STEADY STATE

### What already landed (no further work)

- **CORE-001..008** — the imperative predicates were already retired. `CORE_ID_TABLE` in [builder.rs](./crates/standards/src/framework/builder.rs) has no entry for any of them; the nine `crates/standards/tests/core_parity_*.rs` tests anchor the old behavior as inline reference code (e.g. `core_parity_links_unresolved.rs` "reproduces the *deleted* imperative `check::links::check_markdown_links` body inline"). Declarative `CORE-*` rule files own these checks today.
- **CORE-009** — `rules.namespace-ownership-violation` deliberately stays imperative. As `core_parity_rule_namespace_owner.rs` documents, the declarative `namespace-owner` rule is an intentional smoke-test that does **not** subsume the fused `run_rules_check` predicate (which also owns the `FRAME-*` reservation, dynamic source-adapter owner discovery, and the unknown-owner diagnostic). "No imperative `Check` row is retired by this card."

The result: there is no "Wave 0" duplicate-evaluation left to remove. The remaining imperative predicates emit `CORE-009..051` and have no declarative counterpart.

### Why the remaining burn-down is engine-gated (audited 2026-06)

The migration invariant requires a retiring predicate to reach **parity** with a declarative rule on a fixed fixture — and DECISIONS.md forbids retiring a predicate "by weakening checks." The only declarative hint kinds that can express a *new* check without new Rust are `path-pattern`, line-based `regex`, and `schema`; every fact-consuming kind (`unique`, `cardinality`, `set-coverage`, `set-eq`, `constant-eq`, `content-digest-eq`, `reference-resolves`, `namespace-owner`) is hardcoded to exactly one `source` discriminator, all spent on `CORE-001..009`.

DECISIONS.md lists roughly nine ids as "cleanly migratable author-side" (`CORE-016, 025, 038, 050, 051` via `path-pattern`+`regex`; `035, 036, 044, 047` via `schema`). An audit of the actual predicate bodies, however, found that none of them retire at parity with today's closed kinds:

- **CORE-016** (`HistoryCitation`, [docs_quality.rs](./crates/standards/src/framework/check/docs_quality.rs)) — parses the integer after an `RFC`/`rfc` token and fires only when `number < 100`, to admit standards references like "RFC 3339" / "RFC 5322". The `regex` kind uses the Rust `regex` crate (no lookaround) and matches per line with `find_iter`; it cannot express the numeric threshold or the `RFC-5` vs `RFC-555` boundary.
- **CORE-025** (`OperationalVocabulary`, [prose.rs](./crates/standards/src/framework/check/prose.rs)) — scans `docs`/`plugins`/`.cursor` but **excludes** `docs/explanation/decision-log.md`, `release-notes.md`, `docs/proposals/`, and any `/fixtures/` or `/archive/` segment. `path-pattern` is inclusive-only and cannot express those exclusions; per-line `is_match` also differs from the eval's per-match `find_iter` counting.
- **CORE-050** (`DeclaredToolInvocations`, [tools.rs](./crates/standards/src/framework/check/tools.rs)) — special-cases `specify-contract`, flagging a match only when it is **not** followed by `-validate` (`!line[m.end()..].starts_with("-validate")`). A negative-lookahead condition the lookaround-free, unconditional-per-match `regex` kind cannot express; the candidate set is also a bespoke `active_brief_and_skill_files` walk, not a glob.
- **CORE-035 / 036 / 047** (`ArgumentHintGrammar` / `DescriptionGrammar` / `UnknownTool`, [skill_frontmatter.rs](./crates/standards/src/framework/check/skill_frontmatter.rs)) — these validate skill frontmatter fields (`argument-hint` token grammar, `description` leading imperative verb against a Rust allowlist, `allowed-tools` against a Rust `KNOWN_TOOLS` table plus `mcp__` prefix). Expressing them via `schema` folds them back into the skill JSON Schema that `FrontmatterSchema` (CORE-044) already validates imperatively, producing double emission unless a separate sidecar schema is introduced; the per-token / per-tool finding counts and messages also diverge from a single schema-pattern violation.
- **CORE-044** (`FrontmatterSchema`) and **CORE-051** (`adapter.execution-agent`) are **fused** predicates: `check_schema` emits both `skill.schema-violation` (CORE-044) and `skill.missing-frontmatter` (CORE-042) from one loop; `AdapterCheck` emits both `adapter.execution-agent` (CORE-051) and `adapter.missing-manifest` (CORE-010). Retiring one id means surgically splitting a predicate whose sibling id has no declarative home — exactly the fused-predicate weakening DECISIONS.md cautions against.

So "cleanly migratable author-side" means a CORE rule *file* can be authored — not that the imperative predicate can be retired with parity. The retirement half is engine-gated.

### RFC-31 scope (the path forward for A16)

[RFC-31](https://github.com/augentic/specify/blob/main/rfcs/RFC-31-declarative-lints.md) adds, per migrated predicate class:

1. **`RuleHint.config`** (becoming `RuleHint.config` after the rename step) — per-kind sub-schemas so fact-consuming kinds can express a second metric/policy without new Rust variants for every case.
2. **Extended `regex` and `path-pattern` evaluators** — numeric-capture threshold, negative-match, suffix guards, exclusion globs (Phase 1 lands `regex` config; Phase 2 lands `path-pattern` exclusions).
3. **New `WorkspaceModel` indexer facts** — fence-context, frontmatter granularity, trace-staleness (RFC Phase 2).
4. **De-fusing** — split multi-id predicates before retirement (RFC Phase 2–3).

**Phase 1–2 (complete):** see [`docs/standards/rfc-31-phase1-spike.md`](./docs/standards/rfc-31-phase1-spike.md) and [`docs/standards/rfc-31-phase2-spike.md`](./docs/standards/rfc-31-phase2-spike.md) for engine extensions and binding records.

RFC-31 Phase 4 (2026-06): migratable predicates run via declarative `kind: authoring-predicate` on `CORE-*` rule files; `AuthoringProducer` is CORE-009-only. Do not weaken checks to fake completion.

---

## Done definition

### A19 — done

- [x] No bespoke `authoring::Exit` enum.
- [x] Lint handlers share one `output::run_lint` kernel; no handler-local `println!`/`eprintln!`.
- [x] `Exit::from(&Error)` is the only exit mapping for lint on both binaries.
- [x] [handler-shape.md](./docs/standards/handler-shape.md) documents the lint kernel explicitly.

### A16 — bounded, not closeable today

- [x] CORE-001..008 imperative predicates retired; declarative rules own them.
- [x] CORE-009 retained imperative by design (smoke-test declarative counterpart).
- [x] RFC-31 Phase 4: `CORE_ID_TABLE` is CORE-009-only; `check::run` is namespace bridge only; migratable ids use `authoring-predicate` hints.
- [x] Framework lint no longer runs the full imperative `Check` batch on every `make lint` (declarative pass owns migratable ids). Post-Phase-4 `make lint` on augentic/specify: **~247s** wall (`real 246.75`, 2026-06-04); pre-teardown baseline not captured in-tree.
- [x] RFC-31 Accepted; Phase 1 spike doc at [`docs/standards/rfc-31-phase1-spike.md`](./docs/standards/rfc-31-phase1-spike.md).

---

## Cross-repo touchpoints

| Change | Repository | Files |
| --- | --- | --- |
| Steady-state posture | specify-cli | [DECISIONS.md](./DECISIONS.md) §"Crate layout" |
| Suppression burn-down | specify-cli | [docs/quality-debt.md](./docs/quality-debt.md) |
| Declarative CORE rules (existing) | specify | `adapters/shared/rules/core/CORE-00{1..9}-*.md` |
| Imperative predicate docs | specify | [docs/contributing/checks.md](https://github.com/augentic/specify/blob/main/docs/contributing/checks.md) |
