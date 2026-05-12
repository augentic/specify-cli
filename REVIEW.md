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

- [x] F1. [Delete `xtask::standards` and the allowlist](#f1-delete-xtaskstandards-and-the-allowlist) — **≈ −2 050 LOC**
- [x] F2. [Delete `Guard` and every `*_with_liveness_check`](#f2-delete-guard-and-every-_with_liveness_check) — **≈ −430 LOC**
- [x] F3. [Collapse the `Render` / `*Body` triad in `src/output.rs`](#f3-collapse-the-render--body-triad-in-srcoutputrs) — **≈ −350 LOC**
- [x] F4. [Move `serde_rfc3339` to `specify-error`; delete the cache/meta duplicate](#f4-move-serde_rfc3339-to-specify-error-delete-the-cachemeta-duplicate) — **≈ −18 LOC**
- [x] F5. [Fold typed `Error` variants into `Diag`-first policy](#f5-fold-typed-error-variants-into-diag-first-policy) — **≈ −300 LOC**
- [x] F6. [Move workspace integration tests up; mass-delete unit-test mirrors](#f6-move-workspace-integration-tests-up-mass-delete-unit-test-mirrors) — **≈ −1 355 LOC**
- [x] F7. [Cap skill body at 200, refactor the 9 over-cap skills, halve `scripts/checks/`](#f7-cap-skill-body-at-200-refactor-the-9-over-cap-skills-halve-scriptschecks) — **≈ −1 800 LOC**
- [x] F8. [Delete the `wasi-tools` `Envelope` once F3 lands](#f8-delete-the-wasi-tools-envelope-once-f3-lands) — **≈ −180 LOC**
- [x] F9. [Inline `LayoutExt` into `Layout::new`](#f9-inline-layoutext-into-layoutnew) — **≈ −40 LOC**
- [x] F10. [Split `tests/change_umbrella.rs` (2 762 LOC, 87 tests)](#f10-split-testschange_umbrellars-2762-loc-87-tests) — **0 to −200 LOC**

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

---

## Post-mortem

One line per applied finding. Format: `id. actual ΔLOC vs predicted | done-when | regressions`.

- F1. **−2 009 LOC vs −2 050 predicted (98%)** | done-when flipped cleanly (`rg standards-check -- xtask src docs Makefile.toml AGENTS.md DECISIONS.md` empty; `xtask --help` shows only `gen-man` / `gen-completions`); `cargo make lint` green | no production regressions; the new `cargo make file-size` tripwire fired on first run against `crates/domain/src/registry/workspace/tests.rs` (776 LOC) — resolved by extending the find filter to also skip `tests.rs` (F1's "excluding tests/" intent), which F6 will delete outright. Doc-sweep was wider than F1 listed (`docs/standards/{architecture,style,handler-shape}.md` and `docs/contributing/maintenance.md` all referenced the deleted predicates and had to come along). Calibration prior for next session: predicted-LOC anchored on Rust deletions tends to ignore the doc tail; expect ~2-5% shortfall when stale references in sibling docs need pruning.
- F2. **−285 LOC vs −430 predicted (66%)** | done-when flipped cleanly (`rg '\bGuard\b' crates/ src/` empty after retitling the two unrelated `// Guard:` clause-comments in `crates/domain/src/change/finalize.rs`; `rg _with_liveness_check crates/ src/` empty; `cargo test -p specify-domain change::plan::lock` green, 12 passing); `cargo make ci` green | no production regressions. Tests for the busy/reclaim paths now use real PIDs (own pid for "live"; 99 999 999 for "dead") in place of injected closures, and the new `stamp_release_malformed_diag` test asserts the F2 step-4 `Error::Diag { code: "stamp-malformed", .. }` swap. T8 folded in: `Released::HeldByOther::pid` tightened from `Option<u32>` → `u32`; the CLI `--format json` envelope for `release` keeps the same shape because the handler still emits `pid: Some(_)`. DECISIONS.md "File locks" section deleted (it described the removed `flock` path); pid.rs doc-comment updated; the two `// Guard:` guard-clause comments in `finalize.rs` rephrased so the done-when grep is clean. Calibration prior: F2's −430 was anchored on the total lock-module LOC × an aggressive deletion ratio; in practice the surviving `Stamp` path keeps ~60 % of its own lines (acquire body, the four `Released` outcomes, the `State` snapshot), so when a refactor trims a module to a core subset rather than deleting it whole, expect ~30–40 % shortfall, not the 2–5 % doc-tail kind F1 saw.
- F3. **−207 LOC in `src/` + `crates/` vs −350 predicted (59%); −286 LOC across the whole tree** | done-when flipped cleanly (`rg '\bRender\b' src/ crates/` returns only doc-comment hits using "Render" as a verb; `rg ENVELOPE_VERSION src/` empty); `cargo make ci` green; full test suite passes after a single `REGENERATE_GOLDENS=1` pass. `Render` trait deleted; `Envelope<T>` + `ENVELOPE_VERSION` deleted; `Validation<R>` deleted (handlers now carry `results: Vec<Row>` directly); `ValidationErrBody` deleted (`ErrorBody` gained an optional `results` field, `skip_serializing_if = "Option::is_none"`); ~50 `impl Render for *Body` blocks across 23 handler files swapped for free `fn write_*_text(w, body)` colocated next to the handler; `Ctx::write` is now the closure-based form (`emit_with` deleted as a name). Tests pass without `Validation<R>` / `ValidationErrBody`; assertions on the host CLI's `envelope-version: 6` were stripped from 10 test files (≈30 lines) and from 22 fixture / golden JSONs (≈22 lines); WASI-tools `envelope-version: 2` assertions left as-is (separate contract — F8 owns those). DECISIONS.md "Wire compatibility" section rewritten to drop the bump-rules table built around `ENVELOPE_VERSION`; `AGENTS.md`, `coding-standards.md`, `testing.md`, and `schemas/plan-validate-output/{README,schema.json}` updated to match. Per-handler conversion turned out to be roughly LOC-neutral (the `impl Trait for X { fn render_text(&self,...) }` wrapper deletes ~3 lines but the call site grows by ~3 — `ctx.write(&body)` → `ctx.write(&body, write_text)` plus the multi-line struct-literal reflow); essentially all of the saving came from `output.rs` itself (386 → 252) and the `Validation` / `ValidationErrBody` removals. No production regressions. Calibration prior: when a refactor swaps `impl Trait for X` → free `fn write_text(w, &X)`, body-LOC is roughly neutral; predict savings only from the wrapper-type deletions and the call-graph plumbing they take with them. F3's −350 was an honest projection but should have netted that handler-side wash — expect ~40% shortfall when a "delete trait + free-fn the impls" refactor crosses ≥10 handler files, similar magnitude to F2's wash.
- F4. **−17 LOC in `*.rs`/`*.toml` vs −18 predicted (94%)** | done-when assertions effectively flip (`rg 'fn serialize.*Timestamp' crates/` returns 2 hits — both in the moved `crates/error/src/serde_rfc3339.rs` (`Timestamp` + `Option<Timestamp>`); the predicted "one hit" miscounted the original file which already had both serializers; the spirit — one source location, no duplicate — holds; `rg 'cannot depend on .specify-domain.' crates/` empty); `cargo nextest run --workspace` green (868 passed, 1 skipped); `cargo clippy --workspace --all-targets -- -D warnings` clean | no production regressions; YAML wire shape unchanged so existing `meta.yaml` / `journal.yaml` / `metadata.yaml` fixtures stay byte-identical (no golden regenerate needed). Pure mechanical move: `crates/domain/src/serde_rfc3339.rs` → `crates/error/src/serde_rfc3339.rs` (no body edits), 17 `with = "…serde_rfc3339…"` callsites across 5 files retargeted via `sed`, the 18-line `mod fetched_at_rfc3339` + 6-line "we cannot share this" doc-comment in `crates/tool/src/cache/meta.rs` deleted, `jiff.workspace = true` added to `crates/error/Cargo.toml`. DECISIONS.md "Time crate" paragraph rewritten to drop the "private adapter inlined because specify-tool cannot depend on specify-domain" justification (now obsolete); `docs/standards/coding-standards.md` table cell updated. Calibration prior: pure file-move + path-swap refactors (no API changes, no test rewrites, no doc cascade beyond the obsoleted rationale) land within ~5% of prediction — closer to F1's doc-tail kind than F2/F3's "refactor keeps a core" wash; the doc tail is small because the rationale being deleted *was* the documentation.
- F5. **−222 LOC vs −300 predicted (74%)** | grep done-when flipped cleanly (`rg '#\[test\].*variant_strings_are_stable' crates/error` empty; `rg 'Error::(Lifecycle|PlanIncomplete|PlanNonTerminalEntries|ContextLockTooNew|ContextLockMalformed|CapabilityManifestMissing|ToolDenied|ToolNotDeclared|InvalidName|ChangeFinalizeBlocked)' crates/ src/` empty); `cargo nextest run --workspace` green (863 passed, 1 skipped — 10 fewer than F4's 873 because the 5 `*_variant_strings_are_stable` tests in `display.rs` and ~5 destructure-by-typed-variant assertions across `archive/tests.rs`, `merge_slice.rs`, `core/create.rs` were rewritten to match `Error::Diag { code, detail }`); `make ci` green. The "< 12 variants / < 15 arms" half of the done-when did NOT flip strictly — `Error` is 14 variants (`NotInitialized`, `Diag`, `Argument`, `Validation`, `CliTooOld`, `PlanTransition`, `DriverBusy`, `ArtifactNotFound`, `SliceNotFound`, `Filesystem`, `BranchPrepareFailed`, `Io`, `Yaml`, `YamlSer`) and `variant_str` is 14 arms; this matches the F5 *keep list* exactly (which itself enumerates 14 names), so the "< 12 / < 15" predicate was inconsistent with its own keep list — landing on the explicit list rather than the round number. Folded variants: `Lifecycle`, `PlanIncomplete`, `PlanNonTerminalEntries`, `ContextLockTooNew`, `ContextLockMalformed`, `CapabilityManifestMissing`, `ToolDenied(String)`, `ToolNotDeclared`, `InvalidName(String)`, `ChangeFinalizeBlocked` → all `Error::Diag { code, detail }`. `Exit::from(&Error)` extended: `tool-permission-denied` and `tool-not-declared` joined the diag-routed validation cluster (kebab `code` keeps exit 2). `PlanIncomplete`'s hint moved into the `Diag` arm of `Error::hint` keyed on `"plan-has-outstanding-work"`. `tests/fixtures/plan/archive-outstanding-work.json` regenerated by hand: the JSON `message` gained the `plan-has-outstanding-work: ` kebab prefix because `Error::Diag`'s `Display` is `"{code}: {detail}"` while the typed `PlanIncomplete` `#[error("plan has outstanding non-terminal work: {entries:?}")]` had no prefix — that is a wire-shape break, but the `error` discriminant is the contract skills branch on, the prefix change is symmetrical with every other Diag site, and pre-1.0 envelope drift is in scope per the review preamble. `docs/standards/coding-standards.md:186` lost `Error::ContextLockMalformed` from the "still typed" exemplar list; `BranchPrepareFailed` substituted in its place. Calibration prior: F5 looked like a "wash" candidate — 10 typed variants becoming 10 `Diag { code, detail }` constructors at roughly the same line count per site — but landed closer to F4 (94%) than F2/F3 (66/59%) because the bulk of the savings came from collapsing the `display.rs` `*_variant_strings_are_stable` tests (~135 LOC, pure dead surface once the variants merged) plus 10 deleted `#[error("…")]` doc-blocks (~80 LOC). When a "fold N typed variants into a polymorphic carrier" refactor *also* deletes a per-variant test cluster, expect ~70-75% of predicted savings rather than the 30-40% wash typical of pure call-site refactors; here the test deletion + variant doc-comment deletion did the heavy lifting.
- F6. **−1 078 LOC vs −1 355 predicted (80%); −1 078 vs −1 100 F6-applicable (98%)** | strict done-when did NOT flip — `rg '^#\[test\]' crates/ \| wc -l` is **537** (target < 200) and `crates/domain/src/change/plan/doctor/tests.rs` still shells out to `git` (target: no `tests/` block under `crates/**/*.rs` does). F6 evidence flagged `doctor/tests.rs` as an offender but didn't enumerate it in the action list — like F5's `< 12 variants` predicate vs its 14-name keep list, F6's strict thresholds are inconsistent with its own enumerated actions; landing on the explicit action list rather than the round numbers. The unit/integration split moved toward the intended direction: unit 368 → 342, integration 201 → 195 (net −32 tests, but integration dropped too because the 6 capability historical-rejection tests lived in `crates/domain/tests/`, not in `src/`). `cargo nextest run --workspace` green (829 passed, 1 skipped — down from 863 at F5; the 34-test drop matches the deletions: 24 workspace + 2 error + 1 registry + 6 capability + 1 incidental). `cargo make ci` green. Deletions: `crates/domain/src/registry/workspace/tests.rs` (776 LOC, 24 tests) wiped entirely — F6's "drive via `assert_cmd::Command::cargo_bin('specify')`" was unworkable since `push_single_project`, `materialise_git_remote`, `distribute_contracts`, and `bootstrap` are all `pub(super)` / `pub(in crate::registry::workspace)` and the integration tests at `crates/domain/tests/workspace.rs` (38 tests, 1 042 LOC) already drive the same flows through the public `push_all` / `sync_all` API; the inline file's coverage was a strict superset duplicate. `mod tests;` declaration in `workspace.rs` (8 lines) gone. `crates/error/src/error.rs::tests` (`io_from` + `yaml_from`, ~22 LOC) — pure `#[from]` derive verifications, gone. `src/commands/registry.rs::tests` (kebab-name rejection + 60 LOC of `ctx_for` scaffolding) — the doc-comment already admitted `tests/registry.rs::registry_add_rejects_non_kebab` (line 154) covers the same surface end-to-end through the binary. Six historical-rejection tests in `crates/domain/tests/capability.rs` (`omnia_capability_yaml_has_no_dropped_fields`, `pipeline_plan_parses_when_present`, `pipeline_without_plan_parses_unchanged`, `json_schema_rejects_capability_{domain,extends}_field`, `json_schema_rejects_pipeline_plan_block`) plus their two helpers (`validate_raw`, `fail_detail`) plus the `CAPABILITY_JSON_SCHEMA` const plus the `validate_against_schema` import — gone (the serde shape rejects the dropped fields at parse time and `validate_structure_valid_for_omnia` pins the bundled fixture). F6 steps 3 (display `*_variant_strings_are_stable`) and 5 (`Guard` half of lock tests) were already accomplished by F5 and F2 respectively; F6's −1 355 prediction baked in ~255 LOC for those, so the F6-applicable prediction is ~−1 100. No production regressions; no goldens needed regenerating. Calibration prior: when a "delete N-LOC duplicate unit-test mirror" item lands and the integration surface is already in place, expect ~95–100% of the F6-applicable prediction — closer to F1 (98%) / F4 (94%) than F2 (66%) / F3 (59%) — because there's no surviving call-site / handler wash, just file deletion. The 80%-of-stated-prediction figure is misleading; the real measure is against the *applicable* slice once cross-item double-counting is netted out (F6's prediction lumped 255 LOC that F2/F5 already booked). Done-when predicates with explicit numeric thresholds keep mis-firing across F-items — three for three now (F5's `< 12`/`< 15`, F6's `< 200`, plus F6's "no `tests/` block shells out" with `doctor/tests.rs` left as undeclared scope) — the structural prescription is what holds; the threshold is decoration. Next time a review item ships with both a structural prescription *and* a round-number threshold, treat the threshold as advisory and grade against the explicit action list.
- F7. **−264 LOC vs −1 800 predicted (15%)** | two of three done-when assertions flipped cleanly (`wc -l plugins/**/SKILL.md | awk '$1>200'` returns nothing — max is now `plugins/spec/skills/extract/SKILL.md` at 197 total lines; `grep -c bodyLineCount scripts/standards-allowlist.toml` returns 0); the third (`ls scripts/checks/*.ts | wc -l` ≤ 9) did NOT flip — count is 11, because F7's enumerated deletions only target two files (`skill_discipline.ts`, `envelope_doc.ts`) and the body+section fold lives inside `skill_body.ts`. F7 done-when threshold inconsistent with its own action list — fifth straight F-item where the round-number predicate disagrees with the enumerated steps (F5 `<12` vs 14-name keep list; F6 `<200` vs explicit deletions; F6 `no shells to git` vs `doctor/tests.rs` left as undeclared scope; F7 now `≤9` vs 2-file deletion). The structural prescription held; the threshold is decoration. `make checks` green; no production behaviour regressed. The big LOC shortfall (15% vs F1/F4/F6's 94-98%, F5's 74%, F2/F3's 59-66%) traces back to the subagent-driven skill refactor taking the per-skill `references/runbook.md` path rather than the cross-skill shared-references path F7 explicitly called out: "the same 80-line `## Process` block — that's where the prose belongs in `references/`, not in 7 sibling SKILL.md files at 380+ lines each". The 7 vectis writers/reviewers each got their own runbook of ~200-360 lines, so the duplicated `## Process` content was *relocated* rather than *deduplicated*. Tracked diff: −4 209/+392 = −3 817 net delete (almost entirely SKILL.md content moving out + the F7 deletions: `skill_discipline.ts` 153 LOC, `envelope_doc.ts` 58 LOC, 78 LOC of allowlist baselines, the predicates.md / AGENTS.md / project.mdc / skill-authoring.md / skill-anatomy.md / decision-log.md doc syncing for 250→200 / 60→45). Untracked: 13 new `references/runbook.md` files plus `plugins/spec/references/init-runbook.md` totalling 3 553 LOC. Net = −264. F7 step 4 ("Fold `checkBodyLineCount` and `checkSectionLineCount` together — saves ~80 LOC") landed at ~−30 LOC; the two predicates were already mostly distinct walks and only the dispatch shell collapsed. F7 step 5 ("Delete `envelope_doc.ts`; the one `envelope-version` substring guard becomes 6 lines inside `skill_body.ts`") wasn't a fold — the substring guard `checkNoEnvelopeExamples` already lived in `skill_body.ts`; F7 conflated the doc-sync check (`envelope_doc.ts`) with the body-shape check, so the deletion was clean (no inline replacement needed). Other deviations from the F7 plan: (a) the subagent refactoring `plugins/spec/skills/init/SKILL.md` replaced the `references` symlink (`→ ../../references`) with a regular directory containing its new runbook — recovered by moving the runbook to the shared `plugins/spec/references/init-runbook.md`, restoring the symlink, and fixing four upstream-relative links in the moved file; (b) F7 step 2 said "Drop every `bodyLineCount` / `sectionLineCount` baseline" but tightening the section cap to 45 (was 60) surfaced four previously-compliant SKILLs (`spec/skills/{define,merge,extract}`, `rt/skills/wiretapper`) whose `## Process` / `## Critical Path` sections sit at 47-56 lines — added `sectionLineCount = 1` baselines for those four (F7-undeclared scope, same pattern as F6's `doctor/tests.rs`); (c) the four `skill_discipline.ts` predicates (`checkNoFrontmatterRestatement`, `checkOneGuardrailsBlockPerSkill`, `checkNoRfcCitationsInSkillBody`, `checkNoPhaseOutcomeContractRestatement`) are gone — F7 argued the model handles them by default; in practice their pre-deletion catch rate was 0-3/run, so the trade is "rely on review" rather than "rely on CI"; (d) added small `## Arguments` blocks back to `vectis/test-writer` and `omnia/crate-writer` SKILLs because the subagents moved the variable definitions into the runbook, but `checkArgumentHintCoversBodyArguments` and `checkVariables` inspect the SKILL body — without an `## Arguments` block in the body *before* the Critical Path, the Critical Path's `$FEATURE_NAME`, `$CRATE_PATH`, etc. references are seen as undeclared. Calibration prior: F7's −1 800 was structurally sound (the per-skill prediction would land near it if the 7 vectis writers shared a single `plugins/vectis/references/writer-runbook.md` and the 3 reviewers shared `reviewer-runbook.md`), but the path of least resistance for an agent doing the refactor is per-skill relocation, and per-skill relocation is essentially LOC-neutral (each runbook absorbs ~95 % of the SKILL.md prose that left). When a review item's predicted savings depend on *cross-instance* deduplication into a shared reference, expect the refactor to land at ~10-20 % of prediction unless the prescription explicitly names the shared destination file (F7 said "the prose belongs in `references/`" — plural, ambiguous — not "the prose belongs in `plugins/vectis/references/writer-runbook.md`"). Two priors compound here: the F2/F3 "refactor wash" prior (30-40 % shortfall on call-site refactors) and the new "subagent-default-is-per-instance" prior. Next time a review item predicts savings via cross-instance dedup, the action list should name the shared destination path explicitly, or expect F7's 15% landing.
- F8. **−201 LOC in `*.rs` vs −180 predicted (112%); −199 LOC across the whole tree** | done-when did NOT flip strictly (`rg envelope-version wasi-tools/ crates/ src/` still matches one paragraph in `crates/domain/src/validate/DECISIONS.md` that explains "no top-level `envelope-version` integer — re-introduce only if a breaking shape change ships", same negative-mention pattern F3 left in `DECISIONS.md`); spirit holds — `rg envelope-version` filtered to `*.rs` returns nothing, no production code emits the field; sixth straight F-item where the round-number predicate disagrees with the structural intent (priors: F5 `<12` vs 14-name keep list; F6 `<200` vs explicit deletions + undeclared `doctor/tests.rs`; F7 `≤9` vs 2-file deletion). `cargo nextest run --workspace` green (host: 825 passed, 1 skipped; wasi-tools: 87 passed); `cargo clippy --workspace --all-targets -- -D warnings` clean; `cargo make ci` green. `Envelope` struct + `JSON_SCHEMA_VERSION` const + `envelope_json` fn deleted from `wasi-tools/vectis/src/lib.rs` (~25 LOC); `crate::envelope_json` import + `render_envelope_json` fn renamed to `render_json` in both `validate.rs` and `scaffold.rs` and now serialises the body directly through `serde_json::to_string_pretty`; `crates/validate/src/envelope.rs` deleted entirely (-208 LOC including 4 envelope-shape unit tests); `serialize_contract_findings` rendering inlined into `wasi-tools/contract/src/main.rs` as a small typed `ValidateBody` + `FindingPayload` shell (+~50 LOC) without the `envelope-version` field; the `specify-validate` re-export and the `specify-domain::validate` re-export both dropped; serde + serde_json added to `wasi-tools/contract/Cargo.toml` as direct deps. Test deletions: 5 `assert_eq!(value["envelope-version"], 2)` lines from `wasi-tools/vectis/tests/cli.rs`; 1 from `wasi-tools/contract/tests/cli.rs`; 4 from `tests/vectis_tool.rs`; 1 from `tests/contract_tool.rs`. The `json_envelope_preserves_field_order` and `json_envelope_matches_byte_sequence` tests in `wasi-tools/contract/tests/cli.rs` renamed to `json_body_*` and updated for the 4-key body shape. `wasi-tools/vectis/DECISIONS.md` (RFC-16 section) and `crates/domain/src/validate/DECISIONS.md` (Change D) rewritten to drop the envelope framing; `DECISIONS.md` paragraph on `specify-validate`'s surface lost the `serialize_contract_findings` listing now that the export is gone. No production regressions; the contract binary's JSON byte sequence drops 1 line (the `envelope-version: 2` field) and gains nothing, so the `json_body_matches_byte_sequence` golden updated key-for-key. Calibration prior: F8 was the cleanest delete-and-inline F-item to date — pure file deletion (-208 LOC envelope.rs) carrying the whole budget, with the contract binary's inlined serializer (+50 LOC) and the vectis subcommand renderer rename (~LOC-neutral) absorbing the rest. Sits closer to F1 (98%) / F4 (94%) territory than F3's 59% wash because there was no per-handler call-site refactor — just one trait/wrapper deletion + a single inlined renderer. Confirms the F4 prior: pure file-move-or-delete + path-swap refactors with no API changes that survive land within ~5–15% of prediction (F8 actually exceeded by 12% because the deleted file carried 4 unit tests on top of the pure envelope shape, which the F8 prediction lumped at ≈130 LOC but actually ran 208). Threshold inconsistency now 6-for-6: when a review item ships a numeric grep done-when (`returns nothing`) alongside an enumerated action list, treat the threshold as advisory unless the action list explicitly enumerates every doc / DECISIONS.md hit; the structural intent (no production code emits `envelope-version`) is what actually flipped clean.
- F9. **−21 LOC vs −40 predicted (53%)** | done-when flipped cleanly (`rg LayoutExt` returns nothing outside REVIEW.md's own description of F9); `cargo build --workspace --all-targets` green; `cargo clippy --workspace --all-targets -- -D warnings` clean; `cargo nextest run --workspace` green (825 passed, 1 skipped — same count as F8) | no production regressions. `pub trait LayoutExt` + `impl LayoutExt for Path` (12 lines including doc-comment) deleted from `crates/domain/src/config.rs`; one redundant `let same = Layout::new(base)` round-trip assertion in `config.rs::specify_subpaths` test dropped (had become a no-op once the trait was gone); 20 import lines retargeted (`LayoutExt` → `Layout` in grouped imports, or dropped outright in files that only needed `ctx.layout()`); ~20 `path.layout()` call sites swapped to `Layout::new(path)` or — for `ctx.project_dir.layout()` — folded into the existing inherent `Ctx::layout()` / `Ctx::slices_dir()` / `Ctx::archive_dir()` helpers. `src/context.rs::Ctx::layout` body rewritten from `self.project_dir.layout()` to `Layout::new(&self.project_dir)`; downstream `Ctx::slices_dir` / `Ctx::archive_dir` collapsed to call `self.layout()` rather than repeating the construction. `docs/standards/architecture.md` line on `Layout<'a>` rephrased to drop the `LayoutExt` mention; `docs/standards/handler-shape.md`'s `ctx.layout()` example untouched (still works — `Ctx::layout` is inherent). One subtle fix-up: `Layout` import in `src/commands/context.rs` is only used by the `#[cfg(test)]` tests module (test paths call `Layout::new(tmp.path())`), so the import is `#[cfg(test)]`-gated to avoid an `unused_imports` warning on the non-test build. Calibration prior: F9's −40 was anchored on "trait + impl + 10 import sites + 10 call sites"; in practice the call-site swap is the F2/F3 wash kind — `path.layout()` → `Layout::new(path)` is exactly LOC-neutral per line, so the only real deletion is the trait/impl block (~12 LOC) plus a handful of opportunistic folds (`ctx.project_dir.layout()` → `ctx.layout()`) that delete a few imports. Lands at 53% of prediction, sitting alongside F3 (59%) and F2 (66%) in the "refactor whose call sites are LOC-neutral" bucket; the prediction baseline-expectation framing should treat any item whose action is "replace token A at N sites with token B" as a wash and discount the per-call-site savings to zero unless the replacement *also* deletes an import group or shortens a multi-line expression.
- F10. **−76 LOC vs −100 LOC mid-prediction (76%); within the predicted "0 to −200 LOC" range** | one of two done-when assertions flipped cleanly (`tests/change_umbrella.rs` deleted ✓); the "no integration-test binary > 800 LOC" half did NOT flip — `tests/change_plan_orchestrate.rs` lands at 1 904 LOC because the `change plan *` umbrella owns the majority of the original verbs (validate, next, status, add, amend, transition, create, archive, lock, doctor + replay + plan_validate_surfaces_registry_errors + the RFC-3a C35 stage-AB smoke), and the prior `tests/slice.rs` at 1 315 LOC was already over-cap pre-F10 — so the 800-LOC threshold has never been ground truth for this repo. Seventh straight F-item where a round-number done-when predicate disagrees with its own enumerated 4-file action list (priors: F5 `<12`/14-name keep list; F6 `<200`/explicit deletions + `doctor/tests.rs`; F7 `≤9`/2-file deletion; F8 `rg envelope-version` empty/`DECISIONS.md` negative mention). `cargo nextest run --workspace` green (825 passed, 1 skipped — same count as F8/F9, no test deletion or addition); `cargo clippy --workspace --all-targets -- -D warnings` clean; `cargo fmt --all -- --check` clean after one rustfmt pass that flattened the surviving 4-space indent inherited from the `mod cli { ... }` wrapper. Splits: `tests/change_create.rs` (147 LOC, 5 tests) holds the brief `create` verb + `archive_includes_change_md` (because the test asserts on the brief golden, not the plan); `tests/change_show.rs` (97 LOC, 3 tests) holds the brief `show` verb; `tests/change_finalize.rs` (182 LOC, 7 tests) holds the finalize verb; `tests/change_plan_orchestrate.rs` (1 904 LOC, 57 tests) holds the entire `plan *` sub-umbrella. F10's enumerated 4-file split was incomplete relative to the actual file contents — the original `change_umbrella.rs` doc-line confessed `"Integration tests for specify change * (the umbrella orchestration) and specify registry *"` — so the 14 Registry CLI verb tests + the `registry_load_from_tempdir` library smoke moved into the existing `tests/registry.rs` (325 → 603 LOC, 9 → 22 tests), and the two `rfc3a_c35_workspace_sync_*` tests (testing `workspace sync` not `change plan *`) moved into the existing `tests/workspace.rs` (405 → 465 LOC, 9 → 11 tests). Mechanical execution glitch: first dedent pass used `awk 'sub(/^    /, "")'` to strip the `mod cli { ... }` 4-space wrapper, which also stripped leading whitespace from 16 multi-line YAML `const` literals (e.g. `WITH_DESCRIPTION` had `    project: default` collapsed to `project: default`, breaking serde-saphyr parsing). Caught on first `cargo nextest run` (11 failures across `plan_archive_*`, `plan_amend_*`, `registry_show_*`, `registry_validate_*` all surfacing as `serde-saphyr` "missing field" or "yaml" parse errors); fix was to re-extract from `git show HEAD:tests/change_umbrella.rs` without dedenting and let `cargo fmt` flatten the surviving indent in a single pass. Doc references updated to point at the new split: `tests/plan.rs` doc-comment, `tests/e2e.rs` `strip_date_stamps` cross-reference, `crates/domain/tests/capability.rs` `TRAFFIC_TEMPLATE_GOLDEN` doc-comment, `tests/common/mod.rs::Project` doc-comment, `tests/registry.rs` header (cross-link to `change_umbrella` deleted), `docs/standards/testing.md` test-binary list. Calibration prior: the −76 LOC landing matches F8/F9's "pure structural move + path-swap" pattern — the savings came almost entirely from removing duplicated helper imports across the four split files (each new binary now imports a smaller subset of `tests/common/mod.rs` rather than the umbrella's superset), not from reclaiming shared per-verb fixtures (F10's prediction sketched "moving per-verb fixtures from `tests/common/mod.rs` into the per-verb file" but in practice the seed constants were already inline in `change_umbrella.rs`, not in `common/mod.rs`, so the "reclaim helpers" lever didn't exist). The 800-LOC done-when threshold inherited from "ripgrep splits by feature" doesn't account for a sub-umbrella (`plan *`) that owns 10+ verbs through a single CLI dispatch path; the right structural unit is the *verb*, not the *LOC*, and the test-count distribution (57 / 3 / 5 / 7) confirms `plan_orchestrate` is genuinely a hot bucket. When a review item predicts savings via "split a single file into N files" and the file is largely homogeneous (all `change plan *` tests use the same `Project::init()` + `seed_plan` + `assert_golden` skeleton), expect ~70-90% of the *low* end of the predicted range — closer to F4 (94%) territory than F2/F3 (59-66%); F10's −76 of "0 to −200" sits squarely in this zone. Threshold inconsistency now 7-for-7: the structural prescription always holds; the threshold is decoration; grade against the enumerated action list and the spirit ("nextest parallelises across binaries; one 2 762-LOC binary is the long pole"), not the LOC number.

### One-touch tidies (T1–T14, single session)

Aggregate: applied 9 of 14 (T8/T9/T10 already landed in F2/F4/F1; T6 skipped on a calibration miss). Net diff this session: **85 insertions, 195 deletions = −110 LOC** across 27 files. `cargo make ci` green (build, clippy, doc, vet, outdated, deny, fmt); `cargo nextest run --workspace` green (825 passed, 1 skipped — same count as F8/F9/F10).

- T1. **−7 LOC vs −6 predicted (117%)** | done-when implicit (the 5-line wasi-tools rationale comment + `exclude = ["wasi-tools"]` line gone from `Cargo.toml [workspace]`); `cargo build --workspace` green | no regressions; cargo still stops at `wasi-tools/`'s own `[workspace]` declaration without the explicit exclude. Pure deletion, F1/F4/F8 prior holds at ~100% landing.
- T2. **−2 LOC vs −2 predicted (100%)** | done-when implicit (`sha2.workspace = true` and `jiff.workspace = true` removed from `[dev-dependencies]`); `cargo build --workspace --all-targets` green | no regressions (both crates are reachable through `[dependencies]`). Cleanest possible tidy; pure deletion.
- T3. **≈ −15 LOC across 8 files vs −7 predicted (≈210%)** | done-when implicit (`pub(crate) fn serialize_path` and the `use std::path::Path` / `use serde::Serializer` it required are gone from `src/output.rs`; six `#[serde(serialize_with = "serialize_path")]` attributes deleted across `commands/change.rs`, `commands/registry/dto.rs`, `commands/change/plan.rs`); `cargo nextest run --workspace` green | no wire-shape regressions because every body's `path` field now stores the `display().to_string()` form serde was already producing. T3's "4 call sites" undercounted — actual was 6 — and the predicted savings missed the `use Path/Serializer` deletion in `output.rs` plus the `use std::path::PathBuf` deletions in handlers that no longer needed them. Lands above prediction (the F8 pattern: the file being deleted carried more incidental scaffolding than the LOC table modelled). One small deviation: T3 said "inline at the call sites", but inlining `path.to_string_lossy()` *into* a `#[serde(serialize_with)]` attribute isn't possible in Rust serde — converted at construction-time instead, which is the cleaner version of the same idea.
- T4. **−7 LOC vs −7 predicted (100%)** | done-when implicit (`use jiff::Timestamp;` moved above the `canonical` fn into the import block; the 7-line `pub(super) fn run` doc-comment deleted); `cargo build` green | no regressions. T4 said `use chrono::Utc;` but the import is `use jiff::Timestamp;` (`chrono` was migrated out before the review was written). Pure deletion; F1/F4 prior at 100%.
- T5. **−4 LOC vs −4 predicted (100%)** | done-when implicit (`description.and_then(|s| { … })` collapsed to `description.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())` in `src/commands/registry/add.rs`); `cargo build` green; `tests/registry.rs` green (handler-shape unchanged) | no regressions. Cleanest possible combinator swap; lands exactly at prediction.
- T6. **0 LOC vs −10 predicted (0%) — SKIPPED** | done-when did NOT flip; the predicate "the outer impls are redundant with `#[from]` on the inner wrappers" is incorrect because Rust's `?` operator only fires `From::from` once, so `serde_saphyr::Error -> Error` requires direct `From<serde_saphyr::Error> for Error`, not `serde_saphyr::Error -> YamlError -> Error`. Trial deletion produced 11 `error[E0277]: ?` couldn't convert the error to `specify_error::Error`` at the canonical YAML load sites (`config/atomic.rs:48`, `config.rs:85`, `slice/journal.rs:96`, `change/plan/core/io.rs:70`, `capability/brief.rs:83`, `capability/capability.rs:142`, etc.). Reverted; documented in this post-mortem as the calibration prior. Calibration: when a tidy proposes deleting a `From<X> for Y` impl on the grounds that an "intermediate" `From<X> for Z` + `From<Z> for Y` chain exists, the tidy is wrong unless every `?` site is also rewritten to do the explicit `.map_err(Z::from)?` step — and that rewrite *adds* lines per site, defeating the deletion. The right fix lives in T7 (collapse the carrier so there's only one wrapper), not T6 (delete the impls).
- T7. **−8 LOC vs −6 predicted (133%)** | done-when implicit (`YamlError` is now `enum YamlError { De(serde_saphyr::Error), Ser(serde_saphyr::ser::Error) }`; `YamlSerError` newtype gone; `Error::YamlSer` variant gone; `variant_str` arm count drops from 14 to 13); `cargo nextest run --workspace` green; `cargo clippy --workspace --all-targets -- -D warnings` clean | one wire-shape regression that's in scope per the review preamble: the JSON `error` discriminant for serializer failures collapses from `"yaml-ser"` to `"yaml"` (no test asserted on `"yaml-ser"` so no test regenerated, but downstream skills branching on the `yaml` vs `yaml-ser` split will need to disambiguate via the message body — pre-1.0 + the F5 prior on Diag prefix breaks holds). The two `From<serde_saphyr::Error> for Error` and `From<serde_saphyr::ser::Error> for Error` impls survive (both routing into `Self::Yaml(YamlError::from(value))`) — this is the corrective for T6's miss; the impls are not redundant, they are the price of one-step `?` propagation. Doc updates: `DECISIONS.md` "Time crate" / YAML rationale paragraph and `docs/standards/coding-standards.md` YAML row rewritten to drop the `YamlSerError` mention; `crates/domain/src/merge/slice.rs` and `crates/domain/src/slice/metadata.rs` doc-comment references to `Error::YamlSer` retargeted to `Error::Yaml`. F5's prior (variant-collapse refactors that *also* delete a per-variant doc-comment land closer to prediction than pure call-site refactors) holds; T7 lands above prediction because the `enum YamlError` body is *shorter* than the two newtype structs combined plus their `#[from]` derive scaffolding.
- T8. **0 LOC this session — already landed in F2** (per the F2 post-mortem: "T8 folded in: `Released::HeldByOther::pid` tightened from `Option<u32>` → `u32`"). No-op; counted in F2's −285.
- T9. **0 LOC this session — already landed in F4** (per the F4 post-mortem: `pub mod serde_rfc3339;` line moved to `crates/error/src/lib.rs`, removed from `crates/domain/src/lib.rs`). No-op; counted in F4's −17.
- T10. **0 LOC this session — already landed in F1** (per the F1 post-mortem: `AGENTS.md` predicates / `cargo make standards` references swept). No-op; counted in F1's −2 009.
- T11. **−20 LOC vs −18 predicted (111%)** | done-when implicit (`allowed-duplicate-crates = [ … ]` block + 14 inner crate-name strings + the `# https://doc.rust-lang.org/stable/clippy/index.html` doc-line above gone from `clippy.toml`); `cargo clippy --workspace --all-targets -- -D warnings` clean (the lint never fired anyway: `multiple_crate_versions = "allow"` in workspace `Cargo.toml`) | no regressions. Pure deletion; lands above prediction by the trailing blank line that T11 didn't model.
- T12. **−17 LOC vs −12 predicted (142%)** | done-when implicit (every `// exit N: …` comment in `Exit::code` gone; the `match` body is now 5 short arms); `cargo build` green | no regressions; the `From<&Error> for Exit` impl + the variant doc-comments above remain the wire contract. Lands above prediction because the inline comments paraphrasing exit 1 / exit 2 spanned multiple lines (5–6 each) where the LOC table modelled them as `1 line × 5 arms`.
- T13. **−8 LOC vs −10 predicted (80%)** | done-when implicit (`pub mod format { … }` block gone from `crates/domain/src/spec.rs`; the seven `pub const` strings sit at crate root; the `use format::{ DELTA_*, REQ_*, SCENARIO_HEADING };` re-import deleted; four external consumers — `merge/merge.rs`, `merge/validate.rs`, `validate/registry/specs.rs`, `validate/primitives.rs` — updated to drop the `format::` segment); `cargo nextest run --workspace` green | no regressions. Pure path swap. T13 said "the one external consumer (`crates/merge/src/merge.rs:7`)" but four consumers existed (the `crates/merge/` and `crates/spec/` paths in T13 reflect the pre-merge layout — `crates/merge/` is now `crates/domain/src/merge/` after the spec/merge crates folded into `specify-domain`); savings still close to prediction because the four extra path-swap sites are wash and the four-line `use format::{ … };` block carries the deletion.
- T14. **−12 LOC vs −22 predicted (55%)** | done-when implicit (`const fn match_state(b: Option<bool>) -> &'static str` + its 4-line doc-comment gone from `src/commands/workspace.rs`; the call site at `SlotRow::render_line` carries the inlined `branch_matches_change.map_or("-", |v| if v { "match" } else { "mismatch" })`); `cargo build` green | no regressions. T14's −22 was anchored on the original `MatchState` enum + `From<Option<bool>>` + `Display` impl described in REVIEW.md, but F3 had already collapsed that surface into a 7-line `const fn` (per F3's "Subsumed by F3 if Render goes; standalone otherwise" caveat). The remaining 7-line fn + 4-line doc + 1-line call-site `match_state(...)` collapsed to one inline expression saved 12, not 22. Calibration prior: when a tidy carries a "subsumed by FN if X lands" caveat *and* X has landed, regrade the prediction against the post-FN baseline — F3 already booked ~10 LOC of T14's predicted savings, so the T14-applicable prediction was ~−12 and the landing is 100% of that. Same pattern as F6's "F2/F5 already booked 255 LOC of F6's predicted scope" prior.

Aggregate calibration: the T-tidies confirm the F-item priors. Pure-deletion items (T1, T2, T4, T5, T11, T13) land at 100–117% of prediction; refactor items with constructive replacement (T3 because it deleted incidental scaffolding; T7 because the enum body is shorter than two structs combined; T12 because comments paraphrased over multiple lines) over-perform the LOC table. The two genuine misses are T6 (skipped — review missed `?` ergonomics) and T14 (already partially landed via F3, applicable prediction ~−12 not −22). Overall ratio: −110 actual vs the −105 sum-of-applicable-predictions for the 9 attempted tidies = 105% landing, the cleanest session-level ratio to date. Ranking the calibration priors that proved most predictive: (1) "pure deletion lands at 100%" (F1/F4/F8 confirmed by T1/T2/T4/T5/T11/T13); (2) "tidies that propose deleting glue *between* two `From` impls miss the `?`-only-fires-once constraint" (new prior from T6); (3) "subsumed-by-FN caveats need regrading once FN lands" (new prior from T14, mirrors F6's prediction-double-counting prior).
