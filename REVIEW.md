# Code & Skill Review — execution checklist

Findings from a review of `specify` and `specify-cli` against brevity, maturity-in-restraint, idiomatic Rust, YAGNI, dependencies, tests, and Skills. Items are written to be executed one at a time.

## How to use this list

- Pick **one** item per session. Do not batch.
- Default rule: **a session must not net-add lines to the codebase.** If a refactor adds code, it must delete more elsewhere.
- After each item: re-read the surrounding rules in `AGENTS.md` / `docs/standards/` and delete any paragraph the change made redundant.
- Resist the urge to add a new predicate, check, or doc section to "prevent recurrence". Prevention via mechanical enforcement is what generated the bulk of this list. Trust review + clippy.
- Stop after items 1–3 and re-evaluate before continuing. The downstream items get easier (or vanish) once those land.

---

## Suggested order of attack

Numbered to match the original review's §10. Each links to the detailed entry below.

- [ ] 1. [Delete `xtask::standards` and 90% of the standards allowlist](#1-delete-xtaskstandards-and-the-allowlist)
- [ ] 2. [Delete `Guard`, `with_liveness_check`, `Released::HeldByOther{pid:None}`, and the over-typed `Error` variants](#2-delete-dead-lock-surface-and-collapse-typed-error-variants-into-diag)
- [ ] 3. [Collapse `style.md` + `coding-standards.md` + `handler-shape.md` into one `RULES.md`](#3-collapse-the-standards-docs-into-one-rulesmd)
- [ ] 4. [Make `emit_with` the default; delete the wire-version envelope](#4-make-emit_with-the-default-delete-envelopeversion)
- [ ] 5. [Mass-delete unit tests; move workspace/git tests up to `tests/`](#5-mass-delete-unit-tests-move-workspacegit-tests-up)
- [ ] 6. [Apply 150-line skill cap, drop the skill-cap allowlist, refactor over-cap skills](#6-tighten-skill-caps-and-drop-the-skill-allowlist)

After 6, re-skim §A–§D below. Most are one-touch tidies once the structural items have landed.

---

## 1. Delete `xtask::standards` and the allowlist

**What.** Remove the bespoke standards-check infrastructure: ~1 861 LOC of analyser + 654 lines of toml + the corresponding `cargo make standards` task.

**Why.** Most predicates are zero-baseline post-cleanup tripwires. Reviewer judgment + clippy + a 30-line shell check covers what's left.

**Files to delete.**
- `xtask/src/standards.rs`
- `xtask/src/standards/{allowlist,ast_predicates,crate_root_prose,display_serde_mirror,regex_predicates,report,types,unit_test_serde_roundtrip}.rs`
- `scripts/standards-allowlist.toml`
- The `standards-check` arm in `xtask/src/main.rs`
- `Makefile.toml` `standards` task (and any composing task that depends on it)
- Sections in `docs/standards/predicates.md` (delete the whole file)
- Cross-references in `AGENTS.md`, `docs/standards/coding-standards.md`, `docs/standards/style.md` (`Mechanical enforcement`, `module-line-count`, `direct-fs-write`, etc.)

**What to keep.** A 30-line shell check that fails CI on (a) any `mod.rs` outside `tests/`, (b) any file > 600 lines under `crates/` or `src/`. Live in `Makefile.toml` directly; no Rust code.

**Net change target.** ≥ −2 500 LOC. Add no new mechanical predicates.

**Done when.** `cargo make ci` no longer references `standards-check`; `xtask` either disappears or is reduced to `gen-man` / `gen-completions` only.

---

## 2. Delete dead lock surface and collapse typed `Error` variants into `Diag`

**What.** Two mostly-independent deletions in the same session.

### 2a. Lock surface

- Delete `Guard` and all its helpers: `crates/domain/src/change/plan/lock.rs:23..56`, the corresponding `Guard::*` methods in `crates/domain/src/change/plan/lock/acquire.rs`, `release.rs`, `tests.rs`.
- Delete every `*_with_liveness_check<F>` variant. Inline `is_pid_alive` in the production caller; rewrite the lock tests as integration tests under `tests/lock.rs` that fork a child and let the OS report PID liveness.
- Simplify `Released`: drop `HeldByOther { pid: None }`. The malformed-content path returns `Error::Diag { code: "stamp-malformed", … }`.

### 2b. `Error` variants

For each variant below, replace with `Error::Diag { code: "<kebab>", detail: format!(…) }` at the call site, and delete the variant from `crates/error/src/error.rs` *and* the matching arm in `crates/error/src/display.rs::variant_str`.

Candidates (each has < 3 call sites or no destructuring caller):

- `Error::Lifecycle`
- `Error::PlanTransition`
- `Error::PlanIncomplete`
- `Error::PlanNonTerminalEntries`
- `Error::BranchPrepareFailed`
- `Error::ChangeFinalizeBlocked`
- `Error::ContextLockTooNew`
- `Error::ContextLockMalformed`
- `Error::CapabilityManifestMissing`
- `Error::ToolDenied`
- `Error::ToolNotDeclared`
- `Error::InvalidName`

Verify each before deleting: `rg 'Error::<Variant>'` should show one site (or all sites being constructors that can be replaced with `Diag`).

`Exit::from(&Error)` already handles `Diag`-with-special-code routing (see `src/output.rs:115..125`); extend that arm with any kebab codes that need exit 2.

**Net change target.** ≥ −400 LOC across `crates/error`, `crates/domain`, and the handler call sites. Plus deletion of the corresponding `*_variant_strings_are_stable` tests in `crates/error/src/display.rs`.

**Done when.** `Error` enum is < 12 variants; `display.rs::variant_str` is < 25 arms; all lock tests live under `tests/`.

---

## 3. Collapse the standards docs into one `RULES.md`

**What.** Replace the standards-doc sprawl with a single, short source of truth.

**Inputs.**
- `docs/standards/style.md`
- `docs/standards/coding-standards.md`
- `docs/standards/handler-shape.md`
- `docs/standards/testing.md`
- `docs/standards/predicates.md` (already deleted in §1)

**Output.** One `docs/standards/RULES.md`, **target ≤ 200 lines**, prose-light, example-heavy. Treat `architecture.md` and `DECISIONS.md` as separate concerns and leave them alone.

**`AGENTS.md` becomes a 50-line landing page:** vocabulary, workflow overview, "see `RULES.md`". The current `AGENTS.md` in both repos is already overdue for this.

**Anti-rule.** If two paragraphs say the same thing in different words, pick one and delete the other. Do not add a "and see also" cross-reference. Cross-references are how the docs got here.

**First line of `RULES.md`:**
> Every change to this file deletes or merges an existing paragraph; it does not just add one.

**Done when.** `wc -l docs/standards/*.md AGENTS.md` is < half of today's number, and no rule appears in two places.

---

## 4. Make `emit_with` the default; delete `ENVELOPE_VERSION`

**What.** Two structural simplifications to the output path.

### 4a. `emit_with` as default

- Make `output::write_with` (the closure-based emitter at `src/output.rs:45..49`) the documented default in `RULES.md`.
- Migrate handlers from `*Body + Render + From<&Domain>` to either: (a) `Serialize` directly on the domain type, or (b) `ctx.emit_with(&domain, |w, d| write!(w, ...))`.
- Delete the `Render` trait once no implementors remain.
- Hot list of handlers carrying redundant `*Body` types: `commands/init.rs::Body`+`ContextBody`+`ContextGeneration`, `commands/slice/list.rs::EntryJson`+`TaskCounts`, `commands/change/plan/lock.rs::AcquireBody`+`ReleaseBody`+`StatusBody`. Each is a candidate for direct serialisation of the domain type.

### 4b. Delete the wire envelope

- Delete `output::Envelope<T>`, `ENVELOPE_VERSION`, and `emit_json`'s wrapping logic. Flatten the body.
- Delete the `envelope-version` assertions in `tests/cli.rs` and elsewhere.
- Re-introduce a `v` field if and when 1.0 ships. Today it is bookkeeping with no consumer.

**Net change target.** ≥ −300 LOC across `src/output.rs` and the handler `*Body` types.

**Done when.** `output::Render` is gone (or has < 3 implementors), and `grep -r ENVELOPE_VERSION` returns nothing.

---

## 5. Mass-delete unit tests; move workspace/git tests up

**What.** Recalibrate the unit/integration ratio (currently 580/170) toward integration.

**Delete unit tests that exercise:**
- `From` conversions through `#[from]` (e.g. `crates/error/src/error.rs:240..261`).
- `Display` mirroring of serialised forms (e.g. `crates/error/src/display.rs:77..234`).
- clap argv shape (covered by `tests/cli.rs` against the real binary). Example: `src/commands/registry.rs:73..88` — its own doc-comment admits it's a duplicate of `tests/registry.rs`.

**Move up:**
- `crates/domain/src/registry/workspace/tests.rs` (24 tests, ~700 lines of `Command::new("git")`) → `tests/workspace.rs`. Drive via `assert_cmd` against the binary.

**Keep:**
- Tests of pure-function invariants the type system cannot encode (parsers, transition tables, validators with many edge cases).

**Net change target.** Unit tests < 200; integration tests > 200. Total LOC ≥ −1 500.

**Done when.** `rg '^#\[test\]' crates/ | wc -l` is < 200, and no `tests/` block in a `crates/**/*.rs` source file shells out to `git`/`gh`.

---

## 6. Tighten skill caps and drop the skill allowlist

**What.** Apply the body cap as written and let it bite.

**Steps.**
1. Lower `MAX_BODY_LINES` from 250 → 150 in `scripts/checks/skill_body.ts`.
2. Lower `MAX_SECTION_LINES` from 60 → 40.
3. Delete `bodyLineCount` / `sectionLineCount` entries from `scripts/standards-allowlist.toml`. New baseline is the cap.
4. Refactor the over-cap skills (in priority order):
   - `plugins/omnia/skills/crate-writer/SKILL.md` (currently 377 lines)
   - `plugins/vectis/skills/core-writer/SKILL.md`
   - `plugins/omnia/skills/test-writer/SKILL.md`
   - any other file flagged by the new caps
5. Each refactor moves prose into `references/<topic>.md` and leaves the SKILL.md as: frontmatter + Critical Path (5–7 items) + pointers + one Guardrails block.
6. Consolidate the 28 Deno checks in `scripts/checks/` down to ≤ 8: frontmatter, body cap, section cap, critical-path shape, link integrity, envelope embedding, RFC citation, vocabulary. Delete the others.

**Done when.** No skill exceeds 150 lines; `scripts/checks/` exports ≤ 8 check functions; `scripts/standards-allowlist.toml` has no skill-cap entries.

---

## A. Naming pass (one session, mechanical)

Apply your own `style.md §"Naming by context"` to the offenders. Do all of these in one session, then stop.

- `Error::ChangeFinalizeBlocked` → `FinalizeBlocked`
- `Error::PlanNonTerminalEntries` → `NonTerminalEntries`
- `Error::PlanIncomplete` → `Incomplete`
- `Error::ContextLockMalformed` → `LockMalformed`
- `Error::ContextLockTooNew` → `LockTooNew`
  - (Most of these will already be deleted by §2b — apply the rename only to whatever survives.)
- `RegistryAmendmentProposal` → drop `proposed_` from every field (`src/commands/slice/cli.rs:212..223`)
- `OutcomeKindAction` → `Outcome` (in module `slice::cli`)
- `JournalAction` → `Journal`; `SliceMergeAction` → `Merge`; `SliceTaskAction` → `Task`; `RegistryAction` → `Action` or just expand the variants inline
- `commands::slice::list::collect_status` → `for_slice`; `list_slice_names` → `names`; `status_one` → `one`
- `crates/domain/src/cmd.rs::RealCmd` → `Cmd`

**Done when.** `rg '\bRegistryAmendmentProposal\b'` returns nothing; no `Action` suffix on a single-purpose enum in `src/commands/*/cli.rs`.

---

## B. `Layout` boundary cleanup

**What.** Make `Layout<'a>` the single way handlers talk about the project root.

**Steps.**
- Replace every `Registry::path(project_dir)` / `SliceMetadata::path(project_dir)` / similar with `layout.registry_path()` / `layout.slice_metadata(name)`.
- Move `LayoutExt` extension trait into `Layout`'s `impl` block; delete the trait.
- Change handler signatures so they receive `Layout<'_>`, not `&Path` for the project root.
- Update the relevant paragraph in the new `RULES.md`.

**Done when.** No `&Path` parameter in `src/commands/**/*.rs` represents the project root; `LayoutExt` is gone.

---

## C. Workspace `Cargo.toml` and dep audit

- `cargo tree -i petgraph` — if only `Plan::topological_order` uses it, replace with ~30 lines of Kahn's algorithm in `crates/domain/src/change/plan/core/`.
- Verify `futures-util` is reached via `default` features; if only via `oci`, remove from `[workspace.dependencies]`.
- `glob = "0.3"` — confirm callers; if one or two sites, inline a recursive walk.
- `jsonschema = "0.46"` — heavy dev-dep. If used in only one or two integration tests, replace with `serde_json` + targeted asserts.
- Delete `exclude = ["wasi-tools"]` and its 5-line comment from `Cargo.toml:64..68` (cargo doesn't need it; the comment admits it).
- Reorder `[dependencies]` alphabetically to match the rule documented in (the new) `RULES.md`. Or change the rule.

**Done when.** `cargo tree --duplicates` shows nothing not already documented in `clippy.toml::allowed-duplicate-crates`; `exclude` is gone.

---

## D. Misc one-touch tidies

These are small enough to fold into adjacent PRs.

- `crates/domain/src/lib.rs` — delete the `//! See docs/standards/architecture.md for the rationale.` archaeology line.
- `src/commands/init.rs::canonical` — move `use chrono::Utc;` (currently at line 10) above the `fn canonical` declaration. Delete the 7-line doc on `pub(super) fn run` (lines 21..27); the `debug_assert!` already documents the invariant.
- `src/commands/init.rs::ContextGeneration::skipped` — collapse to `matches!(self, Self::Skipped { .. })`.
- `src/commands/registry/add.rs:35..38` — replace the 6-line `description.and_then(|s| { ... })` with `description.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())`.
- `src/output.rs::Stream` enum + `writer_for(stream)` — collapse to two non-generic functions (`emit_stdout` / `emit_stderr`) or take a `&mut dyn Write` directly. Drops the per-emit `Box<dyn Write>` allocation.
- `src/output.rs::Validation<R>` — single-field struct wrapping a `Vec`. Inline as `#[serde(rename = "results")] Vec<R>` on the parent body, or keep the wrapper but delete its 5-line doc-comment.
- `src/output.rs::serialize_path` — 3-line helper, 7 lines of doc. Inline at the call site.
- `src/output.rs::ValidationErrBody::From<(&Error, &[ValidationSummary])>` — replace with `ValidationErrBody::new(err, results)`.
- `crates/domain/src/change/plan/lock/acquire.rs:38,127` — both `acquire_with_liveness_check` are `pub` but cross no crate boundary. Demote to `pub(crate)` (or delete per §2a).

---

## Anti-checklist

Things this review **deliberately does not propose**, despite the gravitational pull:

- No new `xtask` predicates.
- No new `clippy.toml` overrides.
- No new `*Body` types.
- No new `docs/standards/*.md` files.
- No new `From` impls "for symmetry".
- No new tests for code being deleted.
- No new "Prevention" notes in `AGENTS.md` beyond the deletion-budget rule.

If a session reaches for any of these, stop and reconsider whether the change is necessary.
