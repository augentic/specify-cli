# Diagnostics lint unification plan

Implementation plan for [`augentic/specify` REVIEW.md](https://github.com/augentic/specify/blob/main/REVIEW.md) **Part A Tier 3** items **#16** (imperative→declarative lint burn-down) and **#19** (unify lint output path + `specdev`/`specrun` dispatch).

Scope: `augentic/specify-cli` (primary) and `augentic/specify` (CORE rule files, CI/Makefile, docs). No new product features — structural debt only.

Related docs:

- [DECISIONS.md §"Drained `Error::Validation` and the `Diagnostic` substrate"](../DECISIONS.md#drained-errorvalidation-and-the-diagnostic-substrate)
- [docs/standards/handler-shape.md](./standards/handler-shape.md)
- [adapters/shared/rules/core/README.md](https://github.com/augentic/specify/blob/main/adapters/shared/rules/core/README.md) (framework repo)
- [docs/quality-debt.md](./quality-debt.md) (suppression burn-down tied to A16)

---

## Goals

| Item | Outcome |
| --- | --- |
| **A19** | One lint emit/journal/blocking path; `specdev` and `specrun lint` handlers differ only in pipeline config, not in stdout/stderr/exit plumbing. |
| **A16** | `specdev lint` runs **one** framework scan via the shared indexer + declarative `CORE-*` rules; imperative `framework/check::run` and `AuthoringProducer` are removed. Framework CI time drops roughly in half (REVIEW estimate). |

Non-goals for this plan:

- Collapsing `specrun` and `specdev` into one shipped binary (separate decision).
- Migrating consumer `UNI-*` / target rules to new hint kinds.
- Changing lint vs validate authority (lint stays lifecycle-neutral; validate stays non-silenceable).

---

## Current state (baseline)

### Shared infrastructure already landed

Much of A19 is **partially done**:

- [`crates/standards/src/lint/runner.rs`](../crates/standards/src/lint/runner.rs) — single pipeline: resolve → index → producers → declarative eval → dedupe → ignore pass → envelope.
- [`src/output.rs`](../src/output.rs) — `LintEmit` + `emit_diagnostic_report` shared by both handlers (annotated REVIEW A19).
- Both handlers call `specify_standards::lint::runner::run` with different `PipelineConfig` (profile, producers, resolver degradation, tool runner).

### A19 gaps still open

| Gap | Where | Notes |
| --- | --- | --- |
| Bespoke `Exit` enum | [`src/authoring/exit.rs`](../src/authoring/exit.rs) | Duplicates runtime exit mapping; handler returns `Exit` not `Result<()>`. |
| Manual error paths | [`src/authoring/commands/lint/run.rs`](../src/authoring/commands/lint/run.rs) | `eprintln!`, `emit_fallback_envelope` + raw `println!` on failure; not `output::report`. |
| Handler-shape divergence | `src/authoring/*` vs `src/runtime/commands/lint/run.rs` | Runtime uses `Ctx` + `Result<()>` + `deny_blocking_findings`; authoring uses `Exit` + local `exit_from_error`. |
| Trailing-newline flag | `LintEmit::trailing_newline` | Preserves historical stdout shape; document as wire contract until a semver bump allows normalising. |

### A16 gaps still open

| Gap | Where | Notes |
| --- | --- | --- |
| Double evaluation | `AuthoringProducer` → `check::run` **and** declarative pass for migrated `CORE-*` | Fingerprints dedupe overlap today; work is still duplicated. |
| Double walk | Framework indexer (`ScanProfile::Framework`) **plus** imperative predicates that glob/walk independently via [`framework/context.rs`](../crates/standards/src/framework/context.rs) | Predicates do not consume `WorkspaceModel` today. |
| Imperative registry | [`crates/standards/src/framework/check.rs`](../crates/standards/src/framework/check.rs) | ~30 `Check` implementations still registered in `check::run`. |
| Declarative coverage | `adapters/shared/rules/core/` in specify repo | **9** `CORE-*` rule files on disk (001–008 + 009); **CORE-010..051** ids reserved in [`framework/builder.rs`](../crates/standards/src/framework/builder.rs) but not yet authored. |
| Parity tests | `crates/standards/tests/core_parity_*.rs` | **9** parity tests for migrated rules; pattern to copy for each retirement PR. |
| Unmapped rust/schema predicates | `rust_test_naming.rs`, `rust_source.rs`, `schema_alias.rs` | Not yet in `CORE_ID_TABLE`; need ids + rules or stay as specify-cli-only checks post-burn-down. |

---

## A19 — Unify lint output path and dispatch

### Target architecture

```text
specrun lint run          ─┐
                           ├─► PipelineConfig (surface-specific)
specdev lint              ─┘       │
                                   ▼
                    specify_standards::lint::runner::run
                                   │
                                   ▼
                    emit_diagnostic_report(LintEmit)   ← single tail
                                   │
                    blocking? ──► Result<()> / Exit::from(Error)
```

Both surfaces:

1. Build envelope via shared runner (already true).
2. Emit through `emit_diagnostic_report` (already true on success path).
3. Map infrastructure failures through the same error → exit path as other `specrun` commands.
4. Map blocking findings through `deny_blocking_findings` / `blocking_findings_present` (already shared in emit tail for blocking detection).

### Phase A19-1 — Align failure rendering

**Tasks**

1. Add a small helper in [`src/output.rs`](../src/output.rs) (or `src/lint_emit.rs` if the module grows) for infrastructure failures:
   - Input: `Format`, optional empty `DiagnosticReport`, `&Error`.
   - Behaviour: mirror gate-handler pattern in [handler-shape.md §"Gate handlers render, then fail payload-free"](./standards/handler-shape.md) — render JSON fallback envelope on stdout when `--format json` / `--output-format json`; emit `error: …` on stderr via `output::report` or equivalent.
2. Replace `emit_fallback_envelope` + ad-hoc `eprintln!` in [`src/authoring/commands/lint/run.rs`](../src/authoring/commands/lint/run.rs) with the helper.
3. Ensure `specrun lint run` uses the same helper for any pre-emit abort paths (audit `run.rs` for symmetry).

**Acceptance**

- No raw `println!`/`print!`/`eprintln!` in either lint handler except inside the shared output module.
- Golden tests in `crates/standards/tests/lint_diagnostics_*.rs` still pass; add one integration test asserting JSON failure shape for a forced resolver abort on both surfaces if missing.

### Phase A19-2 — Collapse `specdev` exit mapping

**Tasks**

1. Change [`src/authoring/commands/lint/run.rs`](../src/authoring/commands/lint/run.rs) to return `specify_error::Result<()>` (or delegate to a shared `run_lint(LintRunArgs) -> Result<()>` used by both handlers).
2. Map blocking findings with `specify_standards::lint::ignore::deny_blocking_findings` (same as runtime).
3. Delete [`src/authoring/exit.rs`](../src/authoring/exit.rs); wire [`src/authoring.rs`](../src/authoring.rs) through [`src/runtime/output.rs`](../src/runtime/output.rs) `Exit::from(&Error)` like `specrun`.
4. Document in [handler-shape.md](./standards/handler-shape.md) that `specdev` is a thin binary whose lint handler obeys the same `Result<()>` contract; only bootstrap verbs need bespoke exit subsets.

**Acceptance**

- `Exit` has a single mapping source (`src/runtime/output.rs`).
- `specdev lint` exit codes unchanged: `0` success, `1` generic, `2` blocking/argument (no 3/4 on framework runs).

### Phase A19-3 — Optional shared handler kernel

**Tasks**

1. Extract `fn run_lint(config: LintRunConfig) -> Result<()>` into `src/lint_run.rs` (or `crates/standards` if it stays free of `Ctx`):
   - Owns: timer, `run_pipeline`, `LintEmit`, blocking decision.
   - Callers supply: `ResolveInputs`, `PipelineConfig`, `Layout`, `LintScope`, `command_label`, `trailing_newline`.
2. Reduce [`src/runtime/commands/lint/run.rs`](../src/runtime/commands/lint/run.rs) and authoring lint handler to scope composition + config only.

**Acceptance**

- Handler LOC in each binary tree drops; no behaviour change in `tests/` or `make lint` on specify repo.

### A19 verification checklist

```bash
# specify-cli
cargo make check
cargo test -p specify-standards --test lint_diagnostics_json
cargo test -p specify-standards --test lint_diagnostics_pretty

# specify (sibling checkout)
make lint
```

Wire-contract pins:

- `specdev lint --format json` → stdout `DiagnosticReport` even on infrastructure failure (empty envelope).
- `specrun lint run --output-format json` → same.
- `LintEmit::trailing_newline` behaviour unchanged unless explicitly versioned in release notes.

---

## A16 — Imperative→declarative lint burn-down

### Target architecture

```text
specdev lint
    │
    ▼
lint::runner::run (Framework profile)
    │
    ├─ index::build(ScanProfile::Framework)  ── single walk
    │
    └─ evaluate_rules(CORE-* + UNI-* applicable)  ── no AuthoringProducer
```

Imperative [`framework/check::run`](../crates/standards/src/framework/check.rs) deleted when every predicate either:

- Has a declarative `CORE-NNN` rule + parity test, or
- Is reclassified as specify-cli-only quality (rust predicates) and moved out of the framework lint path.

### Migration invariant (per rule)

For each retiring imperative predicate:

1. Author `adapters/shared/rules/core/CORE-NNN-<slug>.md` in **specify** (follow [core README](https://github.com/augentic/specify/blob/main/adapters/shared/rules/core/README.md)).
2. Add `crates/standards/tests/core_parity_<slug>.rs` proving imperative and declarative findings match on a fixed fixture tree.
3. Remove the `Check` row from `check::run` and delete or gut the predicate module.
4. Remove the `CORE_ID_TABLE` entry in [`framework/builder.rs`](../crates/standards/src/framework/builder.rs) once no imperative code emits that authoring id.
5. Run `make lint` on specify + `cargo make check` on specify-cli in the same PR (cross-repo coordination).

Prefer **one PR per rule cluster** (e.g. all skill-body predicates) to keep reviewable diffs.

### Predicate inventory and suggested waves

#### Wave 0 — Already migrated (declarative owns; imperative still runs)

Retire imperative side in dedicated PRs (parity tests already exist):

| CORE | Imperative id(s) | Parity test |
| --- | --- | --- |
| CORE-001 | `adapter.schema-violation` (retired inline) | `core_parity_adapter_schema.rs` |
| CORE-002 | `links.unresolved-directive` | `core_parity_links_unresolved.rs` |
| CORE-003 | `skill.duplicate-name` | `core_parity_skill_duplicate_name.rs` |
| CORE-004 | `adapter.briefs-cover-operations` | `core_parity_adapter_briefs_cover_operations.rs` |
| CORE-005 | `skill.section-line-count` (body line count) | `core_parity_skill_body_line_count.rs` |
| CORE-006 | `adapter.manifest-version` | `core_parity_adapter_manifest_version.rs` |
| CORE-007 | `adapter.briefs-equal-operations` | `core_parity_adapter_briefs_equal_operations.rs` |
| CORE-008 | `agent-teams.content-digest` | `core_parity_agent_teams_content_digest.rs` |
| CORE-009 | `rules.namespace-ownership-violation` | `core_parity_rule_namespace_owner.rs` |

**Quick win:** Wave 0 alone removes duplicate evaluation for the noisiest cross-file checks and validates the retirement mechanics before harder predicates.

#### Wave 1 — Rules / adapter / agent-teams (CORE-010..012, 051)

| CORE | Module | Imperative rule id |
| --- | --- | --- |
| CORE-010 | `check/adapter.rs` | `adapter.missing-manifest` |
| CORE-011 | `check/agent_teams.rs` | `agent-teams.missing-canonical` |
| CORE-012 | `check/agent_teams.rs` | `agent-teams.non-canonical-overlay` |
| CORE-051 | `check/adapter.rs` | `adapter.execution-agent` |

Hints: mostly `path-pattern`, `set-coverage`, `content-digest-eq`, `constant-eq` (see existing CORE-001..008 as templates).

#### Wave 2 — Links / plugins / brief (CORE-013..022)

| CORE | Module |
| --- | --- |
| CORE-013..014 | `check/brief.rs` |
| CORE-018..020 | `check/links.rs`, `check/schema_links.rs` |
| CORE-021..022 | `check/plugins.rs` |

#### Wave 3 — Docs / prose (CORE-015..017, 023..025)

| CORE | Module |
| --- | --- |
| CORE-015..017 | `check/docs_quality.rs` |
| CORE-023..025 | `check/prose.rs` |

#### Wave 4 — Scenarios (CORE-028..034)

| CORE | Module |
| --- | --- |
| CORE-028..034 | `check/scenarios.rs` |

#### Wave 5 — Skill frontmatter + body (CORE-035..048)

| CORE | Module |
| --- | --- |
| CORE-035..048 | `check/skill_frontmatter.rs`, `check/skill_body.rs` |

Largest wave; split into multiple PRs (frontmatter vs body discipline). While touching `skill_body.rs`, cache regexes with `LazyLock` (REVIEW #16 footnote).

#### Wave 6 — Tools / rules shape (CORE-026..027, 049..050)

| CORE | Module |
| --- | --- |
| CORE-026..027 | `check/rules.rs` |
| CORE-049..050 | `check/tools.rs` |

#### Wave 7 — Specify-cli-only predicates (decision required)

These run inside `check::run` but target the **specify-cli** tree when the framework root is a CLI checkout:

| Predicate | Rule id | Options |
| --- | --- | --- |
| `RustTestNaming` | `rust.test-fn-name-too-long` | (a) Move to `cargo make ci` / separate `specrun` dev command; (b) Author `CORE-052+` with `path-pattern` on `crates/**/*.rs`; (c) Keep as post-burn-down special case. |
| `RustSourceQuality` | `rust.archaeology-in-doc-comment`, `rust.allow-without-reason` | Same. |
| `SchemaAliasCheck` | `schema.alias-hint-kind-parity` | Cross-repo schema parity — likely stays as a dedicated test or becomes CORE with `content-digest-eq` against embedded schemas. |
| `SchemaLinksCheck` | (partial overlap CORE-018) | Merge into CORE-018 or standalone CORE. |

Recommendation: decide in Wave 7 before deleting `check::run`; do not block Waves 0–6 on this.

### Phase A16-final — Remove imperative producer

**Tasks**

1. When `check::run` registry is empty (or only Wave 7 exceptions remain and are moved elsewhere):
   - Remove `AuthoringProducer` from [`src/authoring/commands/lint/run.rs`](../src/authoring/commands/lint/run.rs).
   - Set `PipelineConfig.producers` to `&[]` for framework lint.
   - Change `ResolverDegradation` from `SkipDeclarative` to `Fatal` (framework codex resolution should hard-fail like consumer lint).
2. Delete [`crates/standards/src/framework/check/`](../crates/standards/src/framework/check/) modules that are fully migrated; keep `framework/context.rs` only if still needed for tests.
3. Remove `framework::check` from [`crates/standards/src/framework.rs`](../crates/standards/src/framework.rs); drop module-level `#![allow]` called out in [quality-debt.md](./quality-debt.md).
4. Update [DECISIONS.md](../DECISIONS.md) §crate layout / `AuthoringProducer` paragraph to record retirement.
5. Update specify [checks.md](https://github.com/augentic/specify/blob/main/docs/contributing/checks.md) — imperative `Check` predicates are historical; new framework invariants are `CORE-*` only.

**Acceptance**

- `specdev lint` performs one framework index walk (confirm with a temporary trace counter or benchmark — target ~2× speedup on `make lint`).
- `rg 'AuthoringProducer|framework::check::run' -- crates/ src/` returns no production hits.
- specify CI `make lint` passes with only declarative CORE findings.

---

## Sequencing

```text
A19-1 (failure render) ──► A19-2 (Exit collapse) ──► A19-3 (optional shared kernel)
         │
         └──────────────────────────────┐
                                        ▼
                              A16 Wave 0 (retire dupes)
                                        │
                              A16 Waves 1–6 (CORE authoring)
                                        │
                              A16 Wave 7 decision + A16-final
```

**Rationale**

- Finish A19-1/A19-2 first so migration PRs touch one emit/dispatch path; reduces conflict churn while imperative code is deleted.
- A16 Wave 0 is low-risk proof that parity tests + retirement work before investing in Wave 5 skill-body migration.
- A16-final must not land until at least Wave 0–6 complete (or Wave 7 explicitly scoped out).

Parallelism: A16 waves can proceed in parallel across contributors **after** Wave 0 lands, as long as each PR owns disjoint `CORE-NNN` ids and predicate modules.

---

## Cross-repo touchpoints

| Change | Repository | Files |
| --- | --- | --- |
| New CORE rules | specify | `adapters/shared/rules/core/CORE-*.md` |
| CI entry | specify | `Makefile` (`make lint`), `.github/workflows/*` |
| Schema mirrors | specify | `.cursor/schemas/rule.schema.json` (keep byte-identical to CLI) |
| Parity tests + runner | specify-cli | `crates/standards/tests/core_parity_*.rs`, `framework/check/*` |
| Dispatch / emit | specify-cli | `src/output.rs`, `src/authoring/*`, `src/runtime/commands/lint/*` |
| Decision log | specify-cli | `DECISIONS.md` |
| Review closure | specify | `REVIEW.md` — mark A16/A19 done when acceptance met |

---

## Risks and mitigations

| Risk | Mitigation |
| --- | --- |
| Parity gap — declarative misses edge case imperative caught | Mandatory parity fixture per rule; keep imperative code until parity PR green; overlap period uses fingerprint dedupe. |
| Framework `applicability.artifacts` chassis quirk | Continue using `path-pattern` hints per [core README](https://github.com/augentic/specify/blob/main/adapters/shared/rules/core/README.md) until framework resolver passes `include_unmatched` correctly. |
| stdout shape regression | Pin goldens in `lint_diagnostics_*`; preserve `LintEmit::trailing_newline` until intentional wire bump. |
| Cross-repo PR drift | Single tracking issue listing CORE id → PR links; require both repos green before merge. |
| Wave 5 size | Split by subdirectory (`skill_frontmatter` vs `skill_body`); migrate rules with existing parity scaffolding first (`skill_body.rs` / `scenarios.rs` called out in REVIEW). |

---

## Done definition

### A19 complete when

- [ ] No bespoke `authoring::Exit` enum.
- [ ] Lint handlers use shared failure helper; no handler-local `println!`/`eprintln!`.
- [ ] `Exit::from(&Error)` is the only exit mapping for lint on both binaries.
- [ ] handler-shape.md documents lint handlers explicitly.

### A16 complete when

- [ ] `AuthoringProducer` removed; `specdev lint` uses `producers: &[]`.
- [ ] All Wave 0–6 imperative predicates retired with parity tests.
- [ ] Wave 7 disposition recorded (migrated, moved to CI, or documented exception).
- [ ] `framework/check.rs` registry removed or reduced to documented exceptions.
- [ ] `make lint` on specify measurably faster (team to set target: e.g. ≥40% wall-clock reduction vs baseline).
- [ ] REVIEW.md items #16 and #19 marked addressed.

---

## Suggested first PR (starter scope)

1. **A19-1 + A19-2** in specify-cli only — unify failure emit and collapse `authoring::Exit`.
2. **A16 Wave 0** — one PR removing imperative checks for CORE-001..009 (declarative already owns); nine small deletions + doc touch in specify if any rule docs reference imperative ids.

Estimated size: medium CLI PR + small specify PR (Wave 0 deletions only). Validates the full pipeline before Wave 5 skill-body work.
