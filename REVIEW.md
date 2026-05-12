# Code & Skill Review — execution checklist

Single-pass review of `specify` and `specify-cli` against subtraction, idiom, boundaries, and Skill discipline. Pre-1.0 — back-compat, migrations, and deprecations are not constraints. Items are written to be picked up one at a time by an agent that has not read this file in full.

## Reconnaissance baseline

- `specify-cli`: **281 .rs files / ≈58 510 LOC / 596 `#[test]`** (343 in `crates/`, 169 in `tests/`, 84 in `wasi-tools/`).
- `mod.rs` outside `tests/`: **0** (the `mod-rs-forbidden` predicate has nothing to catch).
- `cargo tree --duplicates`: only the wasmtime/cap-* fan-out, already silenced via `multiple_crate_versions = "allow"`.
- `docs/standards/*.md` + `AGENTS.md` + `DECISIONS.md` = **913 lines**; `scripts/standards-allowlist.toml` = **654 lines / 197 `[file."…"]` blocks** of which **197** are `module-line-count`, 23 `direct-fs-write`, 15 `ritual-doc-paragraphs` — the rest are zero-baseline tripwires.
- Skills in `specify/`: **28 `SKILL.md`**; **9 over the 250-line cap** (top: vectis writers/reviewers at 380+).
- `specify/scripts/checks/`: **3 559 LOC across 13 Deno files**.

## How to use this list

- Pick **one** item per session. Do not batch.
- Default rule: **a session must not net-add lines** to the codebase. If a refactor adds code, it must delete strictly more elsewhere in the same session.
- After each item: re-read the surrounding rules in `AGENTS.md` / `docs/standards/` and delete any paragraph the change made redundant.
- Resist the urge to add a new predicate, check, or doc section to "prevent recurrence". Trust review + clippy.
- Stop after F1–F3 and re-evaluate before continuing. Many downstream items shrink (or vanish) once those land.

---

## Suggested order of attack

Structural items first; one-touch tidies fold into adjacent PRs.

- [ ] F1. [Delete `xtask::standards` and the allowlist](#f1-delete-xtaskstandards-and-the-allowlist) — **≈ −2 050 LOC**
- [ ] F2. [Delete `Guard` and every `*_with_liveness_check`](#f2-delete-guard-and-every-_with_liveness_check) — **≈ −430 LOC**
- [ ] F3. [Collapse the `Render` / `*Body` triad in `src/output.rs`](#f3-collapse-the-render--body-triad-in-srcoutputrs) — **≈ −350 LOC**
- [ ] F4. [Move `serde_rfc3339` to `specify-error`; delete the cache/meta duplicate](#f4-move-serde_rfc3339-to-specify-error-delete-the-cachemeta-duplicate) — **≈ −18 LOC**
- [ ] F5. [Fold typed `Error` variants into `Diag`-first policy](#f5-fold-typed-error-variants-into-diag-first-policy) — **≈ −300 LOC**
- [ ] F6. [Move workspace integration tests up; mass-delete unit-test mirrors](#f6-move-workspace-integration-tests-up-mass-delete-unit-test-mirrors) — **≈ −1 355 LOC**
- [ ] F7. [Cap skill body at 200, refactor the 9 over-cap skills, halve `scripts/checks/`](#f7-cap-skill-body-at-200-refactor-the-9-over-cap-skills-halve-scriptschecks) — **≈ −1 800 LOC**
- [ ] F8. [Delete the `wasi-tools` `Envelope` once F3 lands](#f8-delete-the-wasi-tools-envelope-once-f3-lands) — **≈ −180 LOC**
- [ ] F9. [Inline `LayoutExt` into `Layout::new`](#f9-inline-layoutext-into-layoutnew) — **≈ −40 LOC**
- [ ] F10. [Split `tests/change_umbrella.rs` (2 762 LOC, 87 tests)](#f10-split-testschange_umbrellars-2762-loc-87-tests) — **0 to −200 LOC**

**Total Δ if F1–F10 land cleanly: ≈ −5 800 LOC.** Highest blow-up risk: **F2** — `Guard` is dead, but `Stamp::*_with_liveness_check` is the closure seam most lock tests hang off and the integration replacement (spawn child + real PIDs) is where the day goes sideways.

---

## F1. Delete `xtask::standards` and the allowlist

**What.** Remove the bespoke standards-check infrastructure: `xtask/src/standards{.rs,/*.rs}` (1 257 LOC) + `scripts/standards-allowlist.toml` (654 lines) + the `cargo make standards` task. Keep only a 5-line shell tripwire on file size.

**Evidence.**
- 197 of 198 baseline entries are `module-line-count`; the only non-`module-line-count` predicates with non-zero counts are `direct-fs-write` (23), `ritual-doc-paragraphs` (15), `cli-help-shape` (4), `verbose-doc-paragraphs` (3), `unit-test-serde-roundtrip` (2), `rfc-numbers-in-code` (1), `format-match-dispatch` (1).
- `mod-rs-forbidden` matches **zero** files in the workspace today.
- `Makefile.toml:21,23-25` puts `cargo make standards` on the CI chain.
- ripgrep, fd, helix, jj, cargo all ship with **clippy + a lint script** and no bespoke per-file allowlist.

**Files to delete.**
- `xtask/src/standards.rs`
- `xtask/src/standards/{allowlist,ast_predicates,crate_root_prose,display_serde_mirror,regex_predicates,report,types,unit_test_serde_roundtrip}.rs`
- `scripts/standards-allowlist.toml`
- The `StandardsCheck` arm in `xtask/src/main.rs:24-41,67-88`
- `[tasks.standards]` (and its entry in `[tasks.ci].dependencies`) in `Makefile.toml`
- `docs/standards/predicates.md`
- The `Mechanical enforcement` section + per-predicate links in `docs/standards/coding-standards.md` and `AGENTS.md:42,76`

**What to keep.** A ≤ 8-line gate inline in `Makefile.toml` that fails when any file under `crates/` or `src/` (excluding `tests/`) exceeds 600 lines. No Rust code.

**Net change target.** ≥ −2 050 LOC. Add no new mechanical predicates.

**Done when.** `cargo make ci` no longer references `standards-check`; `xtask` is two subcommands (`gen-man`, `gen-completions`); `rg standards-check -- xtask src docs Makefile.toml AGENTS.md DECISIONS.md` returns nothing.

**Rule?** No. The replacement *is* a 5-line shell check.

**Counter-argument.** "The predicates ratchet down over time, deletion loses signal." Loses because 197 of 202 baselines pin file lengths on files that do not shrink during normal work — they are a no-op tripwire that costs 1 911 LOC + one CI task to maintain.

**Depends on.** none.

---

## F2. Delete `Guard` and every `*_with_liveness_check`

**What.** Two related deletions in the lock module: the unused `Guard` RAII struct and the closure-injected liveness probe.

**Evidence.**
- `rg 'Guard::acquire'` shows **zero non-test, non-self callers**. Production (`src/commands/change/plan/lock.rs:13,25,63`) calls only `Stamp::{acquire,release,status}`.
- `Guard` (`crates/domain/src/change/plan/lock.rs:23-56`), its `impl` (`acquire.rs:14-103`), `Drop` (`release.rs:36-52`), and the corresponding tests are dead surface.
- `Stamp::*_with_liveness_check` exists only so 6 tests can swap in `|_| true` / `|_| false`. Tests can prime a stale-PID file directly (already done in `stale_lock_reclaimed`).

**Total lock module LOC**: 710 (141 + 169 + 26 + 52 + 57 + 265 tests).

**Action.**
1. Delete `pub struct Guard`, `impl Guard`, `impl Drop for Guard`, and every `Guard::*` test.
2. Inline `Stamp::acquire_with_liveness_check` body into `Stamp::acquire`; same for `status_with_liveness_check`. Drop the `is_pid_alive` closure parameter.
3. Rewrite the **8** mock-closure stamp tests to prime an explicit stale lockfile (`fs::write(.., "99999")`) before `Stamp::acquire`. Sites in `crates/domain/src/change/plan/lock/tests.rs`: `stamp_acquire_release` (l. 100), `stamp_reacquire_idempotent` (l. 115), `stamp_acquire_busy` (l. 125), `stamp_reclaims_stale` (l. 138), `stamp_status_absent` (l. 169), `stamp_status_held` (l. 183), `stamp_status_stale` (l. 199), `stamp_status_malformed` (l. 216). The two `stamp_release_*` tests (l. 150, 157) already drive the production path directly and stay as-is.
4. Drop `Released::HeldByOther { pid: None }` (`lock.rs:88-92`); the malformed-content path returns `Error::Diag { code: "stamp-malformed", … }`.

**Net change target.** ≥ −430 LOC.

**Done when.** `rg '\bGuard\b' crates/ src/` returns nothing; `rg '_with_liveness_check' crates/ src/` returns nothing; `cargo test -p specify-domain change::plan::lock` is green.

**Rule?** No.

**Counter-argument.** "`Guard` is reserved for the long-lived driver that's coming." Loses to YAGNI: pre-1.0 + zero callers. Eight lines of `flock` is not where the project will struggle when the long-lived driver lands.

**Depends on.** none.

---

## F3. Collapse the `Render` / `*Body` triad in `src/output.rs`

**What.** Make closure-based emission the only path; delete the wire envelope; delete `Render` once nothing implements it.

**Evidence.**
- `src/output.rs` = 386 LOC. Surface: `Render` trait (6), `Envelope<T>` + `ENVELOPE_VERSION` + `emit_json` (28), `Validation<R>` (15), `ValidationErrBody` (28), `serialize_path` (10), per-handler `*Body` triads across `commands/{init,slice/list,change/plan/lock,codex,…}`.
- `output::emit` (line 225) is a 3-line wrapper around `emit_with`; only reason to keep it is the trait.
- `style.md:34-44` ("One body per command, no wrapper newtype") already says don't introduce `*Body`.
- `Validation<R>` is a single-field wrapper around `Vec<R>`; `ValidationErrBody` re-states `error / message / exit-code` already on `ErrorBody`.
- ripgrep's `crates/printer/src/json.rs` writes one event at a time as a serde-derived struct — no `Render` trait, no envelope wrapper.
- `rg 'envelope-version' specify-cli/tests` returns 0 matches against the host CLI envelope (it is asserted only inside `wasi-tools/*/tests/cli.rs`, which is a separate contract — see F8).

**Action.**
1. Delete `pub trait Render` and `pub(crate) fn write<R: Render>`. Make `write_with` (closure-based) the only success-path entry point.
2. Delete `Envelope<T>`, `ENVELOPE_VERSION`, and `emit_json`'s wrap. Serialise the body directly through `serde_json::to_writer_pretty`.
3. Delete `Validation<R>`. Handlers carry `pub results: Vec<Row>` directly with `#[serde(rename_all = "kebab-case")]`.
4. Delete `ValidationErrBody`. `report` formats validation errors through `ErrorBody` plus a flattened `results` field; one body, one renderer.
5. At each handler, replace `impl Render for Body` with a free `fn write_text(w, body)` colocated with the handler, and call `write_with(format, &body, write_text)`.

**Hot list of handlers carrying redundant `*Body` types**: `commands/init.rs::{Body,ContextBody,ContextGeneration}`, `commands/slice/list.rs::{EntryJson,TaskCounts}`, `commands/change/plan/lock.rs::{AcquireBody,ReleaseBody,StatusBody}`, `commands/codex.rs`.

**Net change target.** ≥ −350 LOC across `src/output.rs` and the per-handler `*Body` triads.

**Done when.** `rg '\bRender\b' src/ crates/` only hits the wasi-tools side; `rg ENVELOPE_VERSION src/` returns nothing.

**Rule?** No. Once the trait is gone the temptation collapses with it.

**Counter-argument.** "JSON consumers expect `envelope-version`." `tests/cli.rs` does not assert it; no skill branches on it. Add a `v` field back at 1.0 if a wire break ever ships.

**Depends on.** none.

---

## F4. Move `serde_rfc3339` to `specify-error`; delete the cache/meta duplicate

**What.** Hoist the bespoke RFC3339-second-precision adapter into the leaf so both downstream crates can share it. Delete the duplicate.

**Evidence.**
- `crates/domain/src/serde_rfc3339.rs` (57 LOC) and `crates/tool/src/cache/meta.rs:251-268` (`mod fetched_at_rfc3339`) are byte-identical.
- The latter's doc-comment confesses the duplication: *"`specify-tool` cannot depend on `specify-domain` (that direction is owned by `specify-domain`)."*
- Both crates already depend on `specify-error` (`crates/tool/Cargo.toml:29`, `crates/domain/Cargo.toml:17`).
- `cargo tree -i jiff` shows it is already in the build graph for both crates.
- Tokio puts shared serde adapters in its leaf time crate — same pattern.

**Action.**
1. `mv crates/domain/src/serde_rfc3339.rs crates/error/src/serde_rfc3339.rs`. Add `pub mod serde_rfc3339;` to `crates/error/src/lib.rs`. Remove the line from `crates/domain/src/lib.rs:13`.
2. Add `jiff = { workspace = true, features = ["serde"] }` to `crates/error/Cargo.toml`.
3. Replace the 12 `with = "specify_domain::serde_rfc3339"` (and `::option`) attributes with `"specify_error::serde_rfc3339"` — matches in `slice/{metadata,journal}.rs`, `commands/slice/{outcome,journal,lifecycle}.rs`. Pure path swap.
4. Delete `mod fetched_at_rfc3339` and the `with = "fetched_at_rfc3339"` attribute in `crates/tool/src/cache/meta.rs`. Replace with `with = "specify_error::serde_rfc3339"`. Delete the 6-line "we cannot share this" doc-comment.

**Net change target.** ≥ −18 LOC (delete the duplicate + the rationale comment).

**Done when.** `rg 'fn serialize.*Timestamp' crates/` returns one hit; `rg 'cannot depend on .specify-domain.' crates/` returns nothing.

**Rule?** No.

**Counter-argument.** "Adding `jiff` to the leaf bloats `specify-error`." It's already in the dep graph for both downstream crates and the feature is gated; zero compile-time impact.

**Depends on.** none.

---

## F5. Fold typed `Error` variants into `Diag`-first policy

**What.** Apply the rule already in `coding-standards.md:178-184` ("Diag-first error policy") and ratchet the `Error` enum from 22 variants to under 12.

**Evidence.** Audit of typed variants without a destructurer or shared shape:

| Variant | Constructors | Destructurers other than `hint` |
|---|---|---|
| `Lifecycle` | 1 | 0 |
| `PlanIncomplete` | 1 | 0 |
| `PlanNonTerminalEntries` | 1 | 0 (handler builds custom envelope) |
| `ContextLockTooNew` | 1 | 0 |
| `ContextLockMalformed` | 1 | 0 |
| `CapabilityManifestMissing` | 1 | 0 |
| `ToolDenied(String)` | 1 | 0 (forbidden by `style.md` "Error variants budgeted by recovery, not source") |
| `ToolNotDeclared` | 1 | 0 |
| `InvalidName(String)` | 1 | 0 |
| `ChangeFinalizeBlocked` | 1 | 0 |

Keep only: `Argument`, `Validation`, `CliTooOld`, `Filesystem`, `BranchPrepareFailed` (key + paths destructured), `Diag`, `Io`, `Yaml`, `YamlSer`, `NotInitialized`, `SliceNotFound`, `ArtifactNotFound`, `DriverBusy`, `PlanTransition` (3+ sites).

**Action.**
1. For each variant in the table, replace the constructor with `Error::Diag { code: "<kebab from variant_str>", detail: format!("…") }`.
2. Extend `Exit::from(&Error)` (`src/output.rs:104-130`) — already pattern-matches on `Diag.code` for the validation cluster — with the kebab codes that previously routed via typed variants.
3. Move each typed variant's hint (`display.rs:21-37`) into the `Diag` arm, keyed by `code`.
4. Delete the corresponding `*_variant_strings_are_stable` tests in `display.rs:93-223`.

**Net change target.** ≥ −300 LOC across `crates/error/src/{error,display}.rs` and the call sites.

**Done when.** `Error` enum is < 12 variants; `display.rs::variant_str` is < 15 arms; `rg '#\[test\].*variant_strings_are_stable' crates/error` returns nothing.

**Rule?** No — the rule is already in `coding-standards.md:178-184`. The deletion is the ratchet.

**Counter-argument.** "Skills grep on `error: lifecycle`." They grep on the kebab `code`, not the variant name; the kebab survives the move.

**Depends on.** none.

---

## F6. Move workspace integration tests up; mass-delete unit-test mirrors

**What.** Recalibrate the unit/integration ratio (currently 343/169 in `crates/` vs `tests/`) toward integration; delete tests that exist only to verify auto-derived plumbing.

**Evidence.**
- `crates/domain/src/registry/workspace/tests.rs` — **24 tests, 776 LOC, all `Command::new("git")` shells** — the canonical "integration test masquerading as unit test".
- `crates/domain/src/change/plan/{core/validate,doctor,lock}/tests.rs` — 24, 22, 17 tests respectively, all driving production paths through `tempdir()`.
- `crates/error/src/error.rs:240-261` — `io_from`, `yaml_from` verify that `#[from]` works.
- `crates/error/src/display.rs:93-223` — six `*_variant_strings_are_stable` tests that exist to assert the wire shape that F5 deletes.
- `src/commands/registry.rs:73-88` — its own doc-comment admits it duplicates `tests/registry.rs`.
- `crates/capability/src/tests.rs` — six pre-1.0 historical-rejection tests guarding *removed* fields: `omnia_capability_yaml_has_no_dropped_fields` (l. 101-126), `pipeline_plan_parses_when_present` + `pipeline_without_plan_parses_unchanged` (l. 170-240), `json_schema_rejects_capability_{domain,extends}_field` + `json_schema_rejects_pipeline_plan_block` (l. 308-386). The `Capability` struct's serde shape rejects these fields at parse time and `validate_structure_valid_for_omnia` (l. 82) catches any reintroduction in the bundled fixture.

**Action.**
1. Move `crates/domain/src/registry/workspace/tests.rs` to `tests/workspace_internal.rs` (or fold into `tests/workspace.rs`). Drive via `assert_cmd::Command::cargo_bin("specify")`.
2. Delete `crates/error/src/error.rs:240-261` (auto-derive verifications).
3. Delete `crates/error/src/display.rs:93-223` after F5.
4. Delete `src/commands/registry.rs:73-88`.
5. After F2, delete the `Guard` half of `crates/domain/src/change/plan/lock/tests.rs` (~120 LOC).
6. Delete the six historical-rejection tests in `crates/capability/src/tests.rs` listed above (~155 LOC). Pre-1.0 + serde + bundled-fixture validation already pin the shape.

**Net change target.** Unit tests < 200; integration tests > 200. Total LOC ≥ −1 355.

**Done when.** `rg '^#\[test\]' crates/ | wc -l` < 200; no `tests/` block under `crates/**/*.rs` shells out to `git`/`gh`.

**Rule?** No. The "lowest external surface" rule is already in `style.md:46-56`.

**Counter-argument.** "Integration tests are slower." `nextest --no-tests=pass` is the default; an additional 24 binary-driven cases adds ≈ 4 s. The 776-LOC inline test file is what slows iteration.

**Depends on.** F2, F5.

---

## F7. Cap skill body at 200, refactor the 9 over-cap skills, halve `scripts/checks/`

**What.** Apply the body cap as written (and tighter), then delete the scripts catching things the model already does by default.

**Evidence.**
- Cap is **250** (`specify/scripts/checks/skill_body.ts:24`); **9 skills exceed it**: vectis/{android-writer 393, ios-reviewer 392, core-writer 391, android-reviewer 389, ios-writer 383, core-reviewer 377, test-writer 341}; omnia/crate-writer 376; omnia/test-writer 234 (just under).
- `specify/scripts/checks/` totals **3 559 LOC across 13 files**.
- `skill_discipline.ts` (153 LOC) catches *"don't restate frontmatter"*, *"use a one-line link for phase outcome"*, *"don't cite RFC numbers in body"*, *"one Guardrails block"* — all things the model already does by default; current CI catches 0–3 violations apiece per run.

**Action.**
1. Lower `MAX_BODY_LINES` to **200** and `MAX_SECTION_LINES` to **45** in `specify/scripts/checks/skill_body.ts:24,33`.
2. Drop every `bodyLineCount` / `sectionLineCount` baseline from `specify/scripts/standards-allowlist.toml`.
3. Refactor over-cap skills in priority of ratchet — vectis first (7 of 9). Each refactor moves long sections under `## Process` / `## References` into `references/<topic>.md` and leaves the SKILL.md as: frontmatter + `## Critical Path` (5–7 entries) + `## Arguments` + one `## Guardrails`.
4. Delete `specify/scripts/checks/skill_discipline.ts`. Fold `checkBodyLineCount` and `checkSectionLineCount` together (one walk, two assertions — saves ~80 LOC).
5. Delete `specify/scripts/checks/envelope_doc.ts`; the one `envelope-version` substring guard becomes 6 lines inside `skill_body.ts`.

**Net change target.** ≥ −1 800 LOC across skills + checks.

**Done when.** `wc -l plugins/**/SKILL.md | awk '$1>200'` returns nothing; `ls scripts/checks/*.ts | wc -l` ≤ 9; `grep -c bodyLineCount scripts/standards-allowlist.toml` returns 0.

**Rule?** Already a rule (`AGENTS.md:79`). The change is enforcement, not addition.

**Counter-argument.** "The vectis writers are large because they describe many platforms." Each platform variant currently restates the same 80-line `## Process` block — that's where the prose belongs in `references/`, not in 7 sibling SKILL.md files at 380+ lines each.

**Depends on.** none.

---

## F8. Delete the `wasi-tools` `Envelope` once F3 lands

**What.** Once the host envelope is gone, drop the WASI-tool sibling for the same reason.

**Evidence.**
- `wasi-tools/vectis/src/lib.rs:36-82` defines `Envelope { envelope_version: u64, … }` with `JSON_SCHEMA_VERSION = 2`.
- `wasi-tools/contract/src/main.rs:44` and `wasi-tools/contract/tests/cli.rs:46,170,210` assert `envelope-version: 2`.
- `src/output.rs:134` pins `ENVELOPE_VERSION: u64 = 6` for the host CLI — different number, different surface.
- `crates/validate/src/envelope.rs` is a third copy of the same pattern (130 LOC).
- `rg 'envelope-version.*== *[1-9]' specify-cli specify` returns nothing — no consumer branches on the version.

**Action.**
1. Delete `Envelope` from `wasi-tools/vectis/src/lib.rs:36-82` and serialise the body directly. Same for `wasi-tools/contract`.
2. Drop the `assert_eq!(value["envelope-version"], 2)` lines from every `wasi-tools/*/tests/cli.rs`.
3. Delete `crates/validate/src/envelope.rs`.

**Net change target.** ≥ −180 LOC.

**Done when.** `rg 'envelope-version' wasi-tools/ crates/ src/` returns nothing.

**Rule?** No.

**Counter-argument.** "WASI tools have an external schema." The schema is `vectis/schemas/*` and `contract/schemas/*`, not the envelope wrap. The wrapper is bookkeeping.

**Depends on.** F3.

---

## F9. Inline `LayoutExt` into `Layout::new`

**What.** Delete the 9-line extension trait whose only justification is `path.layout()` shorthand.

**Evidence.**
- `crates/domain/src/config.rs:213-222` defines `pub trait LayoutExt { fn layout(&self) -> Layout<'_>; }` with a single impl `for Path`.
- `rg 'use.*LayoutExt'` returns **10 sites**, all importing the trait solely for the `path.layout()` syntax.
- `std` does this with bare functions (`Path::canonicalize` is inherent; nothing wraps it in `Ext`).

**Action.**
1. Delete `pub trait LayoutExt { … }` and `impl LayoutExt for Path` (`config.rs:213-222`).
2. Replace each `path.layout()` call site with `Layout::new(path)`.
3. Delete every `use specify_domain::config::LayoutExt;` / `use crate::config::LayoutExt;` import.

**Net change target.** ≥ −40 LOC.

**Done when.** `rg LayoutExt` returns nothing.

**Rule?** No. `style.md:46-56` already says no traits for testability alone; this is a milder variant of the same.

**Counter-argument.** "It's nicer to write `dir.layout().config_path()`." For 10 call sites, the saved 7 characters do not justify the trait + import in every consumer.

**Depends on.** none.

---

## F10. Split `tests/change_umbrella.rs` (2 762 LOC, 87 tests)

**What.** Split the single largest file in the workspace along the verbs it already exercises.

**Evidence.**
- `tests/change_umbrella.rs` = **2 762 LOC / 87 `#[test]`** — by far the largest file. Doc-line: *"Integration tests for `specify change *` (the umbrella orchestration)."*
- `tests/common/mod.rs` = 388 LOC of helpers, much of it shared *only* because everything sits in one binary.
- `nextest` parallelises across binaries; one 2 762-LOC binary is the long pole.
- ripgrep splits its tests by feature (`tests/feature.rs`, `tests/json.rs`, etc.); cargo splits by command.

**Action.** Split along the four orchestration verbs already inside the file: `tests/change_create.rs`, `tests/change_show.rs`, `tests/change_finalize.rs`, `tests/change_plan_orchestrate.rs`. Move per-verb fixtures from `tests/common/mod.rs` into the per-verb file when only that verb uses them.

**Net change target.** 0 to −200 LOC (most of the saving is reclaiming helpers when the artificial sharing dissolves).

**Done when.** `tests/change_umbrella.rs` does not exist; no integration-test binary > 800 LOC.

**Rule?** No. One file > 2 000 lines is one offender, not a pattern.

**Counter-argument.** "Splitting just adds files." Loses because the binary boundary is the unit `nextest` parallelises across.

**Depends on.** none.

---

## One-touch tidies

These are small enough to fold into adjacent PRs.

- T1. **`Cargo.toml:65 exclude = ["wasi-tools"]`** — the 5-line comment admits cargo doesn't need it. Delete both. **−6 LOC.**
- T2. **`Cargo.toml:[dev-dependencies]`** re-lists `sha2` / `jiff` already in `[dependencies]`. Drop the dev re-list. **−2 LOC.**
- T3. **`src/output.rs:384-386 serialize_path`** — 3-line body, 7 lines of doc. Inline at the 4 call sites. **−7 LOC.**
- T4. **`src/commands/init.rs::canonical`** — move `use chrono::Utc;` (line 10) above the `fn`; delete the 7-line doc on `pub(super) fn run` (lines 21-27) — the `debug_assert!` already documents the invariant. **−7 LOC.**
- T5. **`src/commands/registry/add.rs:35-38`** — replace the 6-line `description.and_then(|s| { … })` with `description.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())`. **−4 LOC.**
- T6. **`crates/error/src/error.rs:228-238`** — two manual `From<serde_saphyr::*::Error> for Error` impls that round-trip through the `YamlError` / `YamlSerError` newtypes. With `#[from]` already on the inner wrappers, the outer impls are redundant. Delete. **−10 LOC.**
- T7. **`crates/error/src/yaml.rs`** — collapse `YamlError` and `YamlSerError` into `enum YamlError { De(serde_saphyr::Error), Ser(serde_saphyr::ser::Error) }`. Removes one variant from `Error`. **−6 LOC.**
- T8. **`crates/domain/src/change/plan/lock.rs:73-92` `Released::HeldByOther { pid: Option<u32> }`** — `pid: None` happens only on malformed contents. Per F2, that case becomes `Error::Diag`; the variant becomes `HeldByOther { pid: u32 }`. **−4 LOC.**
- T9. **`crates/domain/src/lib.rs:13`** — the `pub mod serde_rfc3339;` line goes with F4. **−2 LOC.**
- T10. **`AGENTS.md:42,76`** reference `docs/standards/predicates.md` and `cargo make standards` — both go with F1. **−4 LOC.**
- T11. **`clippy.toml:11-27 allowed-duplicate-crates`** is unreachable config. Workspace `Cargo.toml:113` sets `multiple_crate_versions = "allow"`, so the lint never fires and the 17-line allowlist below it never filters anything. Delete the block (and the `# https://doc.rust-lang.org/stable/clippy/index.html` line above it that the rest of the file no longer needs). **−18 LOC.**
- T12. **`src/output.rs:84-105 Exit::code`** — every match arm carries a `// exit N: ...` comment paraphrasing the variant doc immediately above. Drop the inline comments; the variants already document themselves and `From<&Error> for Exit` is the wire contract. **−12 LOC.**
- T13. **`crates/spec/src/lib.rs:13-36 pub mod format`** — the seven `pub const` strings sit in a nested `pub mod format` and are re-imported via `use format::{...}` two lines later. Promote the constants to crate root (keep them `pub`); update the one external consumer (`crates/merge/src/merge.rs:7 use specify_spec::format::REQ_HEADING;`) to drop the `format::` segment. Pure path swap. **−10 LOC.**
- T14. **`src/commands/workspace.rs:277-301 MatchState`** — three-variant enum + `From<Option<bool>>` + `Display` impl exists for one `writeln!` site. Replace with `branch_matches_change.map_or("-", |v| if v { "match" } else { "mismatch" })` inline at the format call. Subsumed by F3 if `Render` goes; standalone otherwise. **−22 LOC.**

---

## Anti-checklist

Things this review **deliberately does not propose**, despite the gravitational pull:

- No new `xtask` predicates.
- No new `clippy.toml` overrides.
- No new `*Body` / `*Row` / `From` impls.
- No new `docs/standards/*.md` files.
- No new "Prevention" notes in `AGENTS.md` beyond the deletion-budget rule.
- No tests for code being deleted.
- No `RULES.md` consolidation as a structural item — it only becomes net-deletion **after** F1 lands and the predicate references go with it. Treat it as a one-touch tidy at that point.
- No drop of `jsonschema` — it is a production dep (`crates/domain/src/capability/capability.rs:353`, `crates/tool/src/validate.rs:525`), not dev-only.

If a session reaches for any of these, stop and reconsider whether the change is necessary.
