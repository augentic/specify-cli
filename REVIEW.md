# Code & Skill Review

**Top three by LOC removed:** F-04 (drop three `slice merge` mirror DTOs, **−60 LOC**), F-05 (delete the duplicated skill-authoring rules from `.cursor/rules/project.mdc`, **−74 LOC**), F-02 (drop `AcquireBody`/`StatusBody` mirrors of `lock::Acquired`/`State`, **−30 LOC**). Total ΔLOC if all eight structural findings land: **≈ −260 LOC** in `specify-cli` plus **≈ −90 LOC** of skill/rules prose. Primary non-LOC axes moved: **−13 mirror DTO types, −10 `From<&Domain>` impls, −3 module-edge files merged**. Most likely to break in remediation: **F-04** — the `MergeOperation` and `BaselineConflict` serde derives must keep `slice merge run|preview|conflict-check` wire-compatible across the three commands, and `MergeOperation` is `#[non_exhaustive]` so any added variant downstream must surface a sensible kebab name.

Reconnaissance numbers used: 207 Rust source files / 29 500 LOC under `crates/` + `src/`; **58 `*Body` / `*Row` structs across 21 files in `src/commands/`** (`rg 'struct .*Body|struct .*Row\b' --type rust src/commands | wc -l` → 58); **10 `From<&...>` impls in `src/commands/`** mostly mapping a domain type to its kebab-mirror; per-file linecounts via `wc -l` quoted below.

---

## Structural findings (ranked)

### F-01 — Drop the `ValidateRow` mirror of `PlanDoctorDiagnostic`

- **Evidence.** `rg -n 'struct ValidateRow' src/commands/plan/lifecycle.rs` →

  ```
  205:struct ValidateRow {
  214:impl From<PlanDoctorDiagnostic> for ValidateRow {
  ```

  The struct's five fields (`level`, `code`, `message`, `entry`, `data`) are field-for-field the domain `Diagnostic` (`crates/domain/src/change/plan/doctor.rs:50-69`), modulo one rename: `severity` → `level`. The single non-trivial construction site (lifecycle.rs:32-40) hand-builds a `ValidateRow` with `level: Severity::Error, code: "registry-shape".to_string(), …`.

- **Action.**
  1. In `crates/domain/src/change/plan/doctor.rs`, the type already derives `Serialize`. Use `Diagnostic` directly.
  2. Delete the `ValidateRow` struct (lines 195-212) and the `From<PlanDoctorDiagnostic>` impl (lines 214-224).
  3. Change `PlanValidateBody.results: &'a [ValidateRow]` to `&'a [Diagnostic]`; rewrite the lone push site to `Diagnostic { severity: Severity::Error, code: "registry-shape".to_string(), message: err.to_string(), entry: None, data: None }`.
  4. `write_validate_row_text` reads `row.severity` instead of `row.level` (one keyword change).
- **Quality delta:** −25 LOC, −1 type, −1 `From` impl, +1 wire rename (`level` → `severity`).
- **Net LOC:** `src/commands/plan/lifecycle.rs` 323 → ~298.
- **Done when:** `rg -n 'struct ValidateRow' src/commands/plan/lifecycle.rs` returns nothing.
- **Rule?** no.
- **Counter-argument.** Skill consumers parse the `level` field. Loses: the review explicitly suspends pre-1.0 back-compat, and the same finding renames the only consumer (`write_validate_row_text`).
- **Depends on.** none.

### F-02 — Drop `AcquireBody` and `StatusBody`, serialize `lock::{Acquired, State}` directly

- **Evidence.** `rg -n 'struct (AcquireBody|StatusBody|ReleaseBody)' src/commands/plan/lock.rs` →

  ```
  69:struct AcquireBody {
  90:struct ReleaseBody {
  112:struct StatusBody {
  ```

  The domain types in `crates/domain/src/change/plan/lock.rs:14-63` (`Acquired { pid, reclaimed_stale_pid, already_held }`, `State { held, pid, stale }`) are field-for-field identical (after dropping `AcquireBody`'s constant `held: true`).
- **Action.**
  1. Add `#[derive(Serialize)] #[serde(rename_all = "kebab-case")]` to `Acquired` and `State` (no behavioural change — domain crate already pulls `serde` from the workspace).
  2. Delete `AcquireBody` (8 LOC) and `StatusBody` (7 LOC).
  3. `ctx.write(&AcquireBody { … }, write_acquire_text)` becomes `ctx.write(&acquired, write_acquire_text)`; same for `state`.
  4. `held: true` on `AcquireBody` is unconditional — drop it from the wire (acquire only returns on success).
- **Quality delta:** −30 LOC, −2 mirror types, −2 inline field plumbings.
- **Net LOC:** `src/commands/plan/lock.rs` 132 → ~102.
- **Done when:** `rg -n 'struct (AcquireBody|StatusBody)' src/commands/plan/lock.rs` returns nothing.
- **Rule?** no.
- **Counter-argument.** Domain types now leak the kebab wire shape. Loses: this is exactly what `Phase`, `Severity`, `Status`, and `LifecycleStatus` already do across the same crate.
- **Depends on.** none.

### F-03 — Drop `EntryRow` mirror of `JournalEntry`, collapse the `Value::String` widening

- **Evidence.** `rg -n 'struct EntryRow|impl From<&JournalEntry>' src/commands/slice/journal.rs` →

  ```
  106:struct EntryRow {
  115:impl From<&JournalEntry> for EntryRow {
  ```

  `JournalEntry { timestamp, step: Phase, r#type: EntryKind, summary, context: Option<String> }` is the domain shape (`crates/domain/src/slice/journal.rs:26-39`). `EntryRow` renames two fields and widens `Option<String>` to `serde_json::Value` purely to switch `if let Some(...)` to `if let Value::String(...)` at the text-render site (lines 88-92).
- **Action.**
  1. On `JournalEntry`, mark `step` with `#[serde(rename = "phase")]` and `r#type` with `#[serde(rename = "kind")]`.
  2. Delete `EntryRow` (10 LOC) and its `From` impl (10 LOC) and the `entries: Vec<EntryRow>` materialisation.
  3. In `write_show_text` use `entry.step`, `entry.r#type`, and `if let Some(context) = &entry.context { ... }`.
- **Quality delta:** −25 LOC, −1 type, −1 `From`, −1 branch (`Value::String` → `Some`), −1 use of `serde_json::Value` at this call site.
- **Net LOC:** `src/commands/slice/journal.rs` 126 → ~101.
- **Done when:** `rg -n 'struct EntryRow' src/commands/slice/journal.rs` returns nothing.
- **Rule?** no.
- **Counter-argument.** `#[serde(rename = "phase")]` on a domain field couples the storage layout to the CLI wire. Loses: the CLI is currently the only serialiser of `JournalEntry`, and the field rename is already paid for in the mirror DTO — just paid one indirection deeper.
- **Depends on.** none.

### F-04 — Drop the three `slice merge` mirror DTOs (`MergedEntry`, `SpecPreviewEntry`, `ConflictRow`)

- **Evidence.** `rg -n 'struct (MergedEntry|SpecPreviewEntry|ConflictRow)|impl From<&(MergePreviewEntry|BaselineConflict)>' src/commands/slice/merge.rs` →

  ```
  122:struct MergedEntry {
  129:impl From<&MergePreviewEntry> for MergedEntry {
  171:struct SpecPreviewEntry {
  179:impl From<&MergePreviewEntry> for SpecPreviewEntry {
  217:struct ConflictRow {
  225:impl From<&BaselineConflict> for ConflictRow {
  ```

  Three rows × two-to-four-field projections of `MergePreviewEntry` / `BaselineConflict`. Each costs a struct + a `From` (~18 LOC each).
- **Action.**
  1. Add `Serialize, rename_all = "kebab-case"` to `MergePreviewEntry` and `BaselineConflict` in `crates/domain/src/merge/`.
  2. For `baseline_path: PathBuf`, attach `#[serde(serialize_with = "serialize_path_display")]` (one tiny helper — same scope as the existing `specify_error::serde_rfc3339`).
  3. For `BaselineConflict.baseline_modified_at`, use `#[serde(with = "specify_error::serde_rfc3339")]` (already in the workspace).
  4. Delete `MergedEntry`, `SpecPreviewEntry`, `ConflictRow`, and the three `From` impls (≈ 60 LOC).
  5. `RunBody.merged_specs: Vec<MergedEntry>` and friends switch to `&[MergePreviewEntry]` / `&[BaselineConflict]`.
- **Quality delta:** −60 LOC, −3 mirror types, −3 `From` impls.
- **Net LOC:** `src/commands/slice/merge.rs` 406 → ~346.
- **Done when:** `rg -n 'struct (MergedEntry|SpecPreviewEntry|ConflictRow)' src/commands/slice/merge.rs` returns nothing.
- **Rule?** no.
- **Counter-argument.** `MergeOperation` is `#[non_exhaustive]` and its serialisation would now travel through the public wire of three CLI verbs. Loses: the variants already round-trip through `MergeOperation` derived `Serialize` on the domain side (`merge.rs:240-255` already matches them by name).
- **Depends on.** none.

### F-05 — Delete duplicated skill-authoring rules from `.cursor/rules/project.mdc`

- **Evidence.** `wc -l docs/standards/skill-authoring.md .cursor/rules/project.mdc` (in the `specify` repo) → `121` and `311` lines. `rg -n '^### (Frontmatter shape|name|description|argument-hint|Critical Path|Body length|Validation)$' .cursor/rules/project.mdc` →

  ```
  223:### Frontmatter shape
  246:### `name`
  253:### `description`
  260:### `argument-hint`
  271:### Critical Path
  277:### Body length
  283:### Validation
  ```

  Lines 219-296 of `.cursor/rules/project.mdc` restate every rule already in `docs/standards/skill-authoring.md` (description grammar, argument-hint grammar, 200/45/512 caps, name regex, forbidden frontmatter list, Critical-Path discipline). The rule file even *says* it: line 221 "The long-form rationale lives under `## Rationale` in `docs/standards/skill-authoring.md`" — and then duplicates the rules anyway.
- **Action.**
  1. In `.cursor/rules/project.mdc`, replace the entire `## Skill authoring conventions` section (lines 219-296) with a three-line pointer:

     ```markdown
     ## Skill authoring conventions

     Every `SKILL.md` follows the house style in [docs/standards/skill-authoring.md](../../docs/standards/skill-authoring.md). Predicate sources: [scripts/checks/](../../scripts/checks/); schema: [.cursor/schemas/skill.schema.json](../../.cursor/schemas/skill.schema.json). `make checks` enforces both.
     ```
  2. Keep `docs/standards/skill-authoring.md` as the single source of truth.
- **Quality delta:** −74 LOC of duplicated prose, −1 source of truth.
- **Net LOC:** `.cursor/rules/project.mdc` 311 → ~237.
- **Done when:** `wc -l .cursor/rules/project.mdc` shows ≤ 240, and `rg -n '^### Frontmatter shape' .cursor/rules/project.mdc` returns nothing.
- **Rule?** no.
- **Counter-argument.** The rule file is always-loaded; consolidating to a link defers a request whenever the agent needs the rules. Loses: Stage-1 metadata is precious (per the skill-authoring doc itself), and one link plus `make checks` is the discipline already in place.
- **Depends on.** none.

### F-06 — De-duplicate the 13-step algorithm in `change-execute` SKILL.md

- **Evidence.** `rg -n '^## Critical Path|^## Per-slice algorithm at a glance' plugins/change/skills/execute/SKILL.md` →

  ```
  6:## Critical Path
  77:## Per-slice algorithm at a glance
  ```

  `wc -l plugins/change/skills/execute/SKILL.md` → 145. Lines 6-14 list the seven-step driver loop (resolve root, acquire lock, self-heal, pick next, prepare workspace, run phases, wrap up). Lines 79-95 list the same 13 steps with one extra layer of detail. `docs/standards/skill-authoring.md` line 49 explicitly forbids this: *"duplicating both forms in the same body is the anti-pattern this rule eliminated."*
- **Action.**
  1. Delete the `## Per-slice algorithm at a glance` H2 (lines 77-97 in the body). The summary bullets are already in Critical Path; the full algorithm is in `per-slice-algorithm.md`.
  2. Move the one sentence from line 97 ("`outcome.summary` is copied byte-for-byte into `--reason` at steps 11c and 12c. Never paraphrase.") into the existing `## Guardrails` H2 next to the equivalent bullet at line 126.
- **Quality delta:** −20 LOC, fixes one mechanically-documented anti-pattern.
- **Net LOC:** 145 → ~125.
- **Done when:** `rg -c '^## Per-slice algorithm at a glance' plugins/change/skills/execute/SKILL.md` returns 0.
- **Rule?** no — `docs/standards/skill-authoring.md` already declares the rule; `make checks` ostensibly already enforces it (`checkOneCriticalPathForm`); the finding is to make this file conform.
- **Counter-argument.** The expanded list helps a returning operator. Loses: that operator's home is `per-slice-algorithm.md`, which the Critical Path already links to.
- **Depends on.** none.

### F-07 — Merge `crates/domain/src/change/plan/lock/{acquire,release,status,pid}.rs` into the parent `lock.rs`

- **Evidence.** `wc -l crates/domain/src/change/plan/lock.rs crates/domain/src/change/plan/lock/*.rs` →

  ```
   93 crates/domain/src/change/plan/lock.rs
   62 crates/domain/src/change/plan/lock/acquire.rs
   27 crates/domain/src/change/plan/lock/pid.rs
   42 crates/domain/src/change/plan/lock/release.rs
   43 crates/domain/src/change/plan/lock/status.rs
  ```

  Each impl-side file carries its own `use std::fs;` / `use std::path::Path;` / `use specify_error::Error;` / `impl Stamp { fn … }` shell. Four files, four module declarations in `lock.rs`, ≈ 12 lines of duplicate `use` headers, zero internal cohesion gained — the four functions are tiny and all carry `impl Stamp`.
- **Action.**
  1. Inline `acquire.rs`, `release.rs`, `status.rs`, and `pid.rs` into the parent `lock.rs`. Tests stay in `lock/tests.rs`.
  2. Drop `mod acquire; mod pid; mod release; mod status;` from `lock.rs` lines 6-9.
- **Quality delta:** −15 LOC (deduped imports + module declarations), −4 module-edge files, no change to surface.
- **Net LOC:** 5 files × 267 LOC → 1 file × ~252 LOC.
- **Done when:** `rg --files crates/domain/src/change/plan/lock/ | wc -l` is 1 (`tests.rs`).
- **Rule?** no.
- **Counter-argument.** Per-verb files make stack traces easier to read. Loses: `cargo` shows function names in traces, not module paths, and 174 LOC across four files is the wrong shape for "scale-out" anyway.
- **Depends on.** none.

### F-08 — Drop `OverlapRow` mirror in `slice/touched.rs`

- **Evidence.** `rg -n 'struct OverlapRow|impl From<&specify_domain::slice::Overlap>' src/commands/slice/touched.rs` →

  ```
  126:struct OverlapRow {
  133:impl From<&specify_domain::slice::Overlap> for OverlapRow {
  ```

  The mirror swaps two field names (`other`→`other_slice`, `ours`→`our_spec_type`, `theirs`→`other_spec_type`) and calls `to_string()` on two strum-derived enums that already serialise as kebab strings.
- **Action.**
  1. Add `#[derive(Serialize)] #[serde(rename_all = "kebab-case")]` to `specify_domain::slice::Overlap` and rename the three fields to the desired wire names *in the domain type* — this is a pre-1.0 rename, no migration needed.
  2. Delete `OverlapRow` (8 LOC) and the `From` impl (10 LOC).
  3. `overlaps: Vec<OverlapRow>` in `OverlapBody` becomes `overlaps: &[Overlap]`.
- **Quality delta:** −20 LOC, −1 type, −1 `From`, two `.to_string()` calls deleted on the hot path.
- **Net LOC:** `src/commands/slice/touched.rs` 143 → ~123.
- **Done when:** `rg -n 'struct OverlapRow' src/commands/slice/touched.rs` returns nothing.
- **Rule?** no.
- **Counter-argument.** `Overlap` field renames cascade through any test that names the fields. Loses: integration tests assert wire shape (`overlaps[i].other_slice == "x"`); the rename brings those into line.
- **Depends on.** none.

---

## One-touch tidies

### T-01 — Drop `PathRef` wrapper in `change.rs` and `plan/create.rs`

- **Evidence.** `rg -n 'struct PathRef\b' src/commands` →

  ```
  src/commands/change.rs:159:struct PathRef {
  src/commands/plan/create.rs:186:struct PathRef {
  ```

  Two identical `struct PathRef { path: String }` definitions. Each wraps a string into `{ "path": "..." }` in the wire envelope for no reason — there is no peer key.
- **Action.** Inline: `plan: PathRef { path: plan_path.display().to_string() }` → `plan: plan_path.display().to_string()`; change `CreateBody.plan: PathRef` → `CreateBody.plan: String`. Drop both struct definitions.
- **Quality delta:** −16 LOC, −2 types.
- **Done when:** `rg -n 'struct PathRef\b' src/commands` returns nothing.

### T-02 — Drop `CreateIfExistsArg` — derive `clap::ValueEnum` on `CreateIfExists` directly

- **Evidence.** `rg -n 'clap = ' crates/domain/Cargo.toml` confirms `clap` is already a domain dep (line 20). `rg -n 'enum CreateIfExists\b|enum CreateIfExistsArg' --type rust` →

  ```
  crates/domain/src/slice/actions/create.rs:15:pub enum CreateIfExists {
  src/commands/slice/cli.rs:290:pub enum CreateIfExistsArg {
  ```

  The mirror enum plus its `From` cost ~18 LOC for three identically-named variants.
- **Action.** Add `clap::ValueEnum` to the `CreateIfExists` derive list (next to the `Phase` enum precedent in `capability.rs:96`). Delete `CreateIfExistsArg` (8 LOC) and the `From` impl (9 LOC) in `src/commands/slice/cli.rs`.
- **Quality delta:** −17 LOC, −1 enum, −1 `From`, −1 module edge.
- **Done when:** `rg -n 'enum CreateIfExistsArg' src/commands/slice/cli.rs` returns nothing.

### T-03 — Drop `CreateBody` (slice/lifecycle.rs) and its `From<&Created>` impl

- **Evidence.** `rg -n 'struct CreateBody|impl From<&Created>' src/commands/slice/lifecycle.rs` →

  ```
  39:struct CreateBody {
  48:impl From<&Created> for CreateBody {
  ```

  The `From` impl is field plumbing only — `name` is `dir.file_name()`, `slice_dir` is `dir.display()`, the rest are `metadata.*` reads (12 LOC).
- **Action.** Add `Serialize` to `Created` with a `display_path` serde adapter for `dir`; flatten `metadata` via `#[serde(flatten)]`. Delete `CreateBody` + `From`.
- **Quality delta:** −22 LOC, −1 type, −1 `From`.
- **Done when:** `rg -n 'struct CreateBody' src/commands/slice/lifecycle.rs` returns nothing.

### T-04 — Collapse `cli.rs` `pub use … cli::*Action` re-exports

- **Evidence.** `rg -n 'pub use crate::commands::.*::cli::' src/cli.rs` →

  ```
  10:pub use crate::commands::capability::cli::CapabilityAction;
  11:pub use crate::commands::change::cli::ChangeAction;
  12:pub use crate::commands::codex::cli::CodexAction;
  13:pub use crate::commands::compatibility::cli::CompatibilityAction;
  14:pub use crate::commands::context::cli::ContextAction;
  15:pub use crate::commands::plan::cli::{LockAction, PlanAction};
  16:pub use crate::commands::registry::cli::RegistryAction;
  17:pub use crate::commands::slice::cli::{ … };
  21:pub use crate::commands::tool::cli::ToolAction;
  22:pub use crate::commands::workspace::cli::WorkspaceAction;
  ```

  11 cross-module re-exports. The only consumer is `src/commands.rs`, which can import directly.
- **Action.** Delete the 11 `pub use` lines. In `src/commands.rs`, change `use crate::cli::{ … }` to import from the canonical paths (one extra `use` per submodule, paid in the file that already imports those modules).
- **Quality delta:** −11 LOC, −7 module-edge re-exports.
- **Done when:** `rg -c '^pub use crate::commands::' src/cli.rs` returns 0.

### T-05 — Drop `Row` mirror in `slice/outcome.rs`

- **Evidence.** `rg -n 'struct Row\b|impl From<&specify_domain::slice::Outcome>' src/commands/slice/outcome.rs` →

  ```
  167:struct Row {
  178:impl From<&specify_domain::slice::Outcome> for Row {
  ```

  Same pattern as F-01/F-03/F-08.
- **Action.** Serialize `Outcome` directly; drop the mirror.
- **Quality delta:** −18 LOC, −1 type, −1 `From`.
- **Done when:** `rg -n 'struct Row\b' src/commands/slice/outcome.rs` returns nothing.

### T-06 — Inline `crates/error/src/display.rs` (78 LOC) back into `error.rs`

- **Evidence.** `wc -l crates/error/src/display.rs crates/error/src/error.rs` → `78` and `172`. `rg -n '^pub mod display' crates/error/src/lib.rs` → line 5. `display.rs` contains exactly two `impl Error { fn … }` methods (`hint`, `variant_str`) — no shared private state, no types of its own.
- **Action.** Paste the two impl blocks into `error.rs`, delete `display.rs`, remove `pub mod display;` from `lib.rs`.
- **Quality delta:** −6 LOC (file header + `mod` declaration + duplicate `use Error`), −1 module-edge file.
- **Done when:** `rg --files crates/error/src/ | wc -l` shows 5 (or 4 if the same pass swallows `yaml.rs`).

### T-07 — Drop `change.rs` `BriefShowBody { brief, path }` — `path` is already inside the body text

- **Evidence.** `rg -n 'struct BriefShowBody' src/commands/change.rs` → line 170. `BriefShowBody` is 5 LOC; serializing `Option<ChangeBrief>` directly (which already derives `Serialize`) plus passing `&path` into `write_brief_show_text` via a separate argument removes the wrapper.
- **Action.** Remove the struct; the text writer takes `(brief, path)` as two arguments. `ctx.write` becomes a manual two-step (call `write` on `&brief`; print the path header before/after as plain text); or keep a one-time inline tuple body.
- **Quality delta:** −10 LOC, −1 type.
- **Done when:** `rg -n 'struct BriefShowBody' src/commands/change.rs` returns nothing.

### T-08 — `workspace.rs` `StatusBody::Absent {}` — drop the `#[expect(clippy::empty_enum_variants_with_brackets)]` ceremony

- **Evidence.** `rg -n '#\[expect\(.*empty_enum_variants_with_brackets' src/commands/workspace.rs` →

  ```
  181:#[expect(
  182:    clippy::empty_enum_variants_with_brackets,
  ```

  The reason in the attribute is "keep `Absent` as `{}` on the wire, not `null`". With `#[serde(tag = "kind", rename_all = "kebab-case")]` on `StatusBody`, both variants become `{ "kind": "absent" }` / `{ "kind": "present", "slots": [...] }` — the `Absent {}` shape is no longer needed and the `expect` attribute disappears.
- **Action.** Replace `#[serde(untagged)]` with `#[serde(tag = "kind", rename_all = "kebab-case")]`, change `Absent {}` to `Absent`, drop the 5-line `#[expect(...)]` block. Adjust the text-render match accordingly.
- **Quality delta:** −7 LOC, −1 `#[expect]` attribute, −1 untagged-enum match-arm gotcha.
- **Done when:** `rg -n '#\[expect\(.*empty_enum_variants' src/commands/workspace.rs` returns nothing.

---

## Post-mortem

- **F-01.** Applied. Actual ΔLOC `−36` (323 → 287) vs predicted `−25`; the review undercounted by missing the `.into_iter().map(ValidateRow::from).collect()` chain that collapsed to a direct `plan_doctor(...)` assignment. "Done when" (`rg 'struct ValidateRow' src/commands/plan/lifecycle.rs` → nothing) flipped cleanly. Wire rename `level` → `severity` propagated to `schemas/plan-validate-output/{schema.json,README.md}`, `tests/fixtures/plan/validate-duplicate-name.json`, and four `["level"]` lookups in `tests/plan_orchestrate.rs`; full `cargo test` + `cargo clippy --all-targets -- -D warnings` clean, no regressions.
- **F-02.** Applied. Actual ΔLOC `−32` (132 → 100) vs predicted `−30`; "Done when" (`rg 'struct (AcquireBody|StatusBody)' src/commands/plan/lock.rs` → nothing) flipped cleanly. Wire change: `acquire` body no longer carries the redundant `held: true` constant — `tests/plan_orchestrate.rs::plan_lock_acquire_release_cycles` now asserts `acquired.get("held") == None` instead. `Acquired` and `State` derive `Serialize` + `rename_all = "kebab-case"` directly in the domain; full `cargo test --all-targets` + `cargo clippy --all-targets -- -D warnings` clean, no regressions.
- **F-03.** Applied. Actual ΔLOC `−20` (126 → 106) vs predicted `−25`; the gap is the `ctx.write(&ShowBody { name, entries: journal.entries }, write_show_text)?` site reformatting to a four-line builder under `rustfmt`. "Done when" (`rg 'struct EntryRow' src/commands/slice/journal.rs` → nothing) flipped cleanly. Wire/storage rename: `JournalEntry.step` and `r#type` now serialise as `phase`/`kind` on both the on-disk `journal.yaml` and the `slice journal show --format json` body — `crates/domain/src/slice/journal.rs::append_persists_to_disk_and_load_returns_entry` and `tests/slice.rs::journal_append_writes_to_file` updated to expect `phase:`/`kind:` (was `step:`/`type:`); the existing `journal_show_empty_then_populated` already asserted `entries[0]["phase"]`/`["kind"]` so no JSON-side updates were needed. Pre-1.0 storage rename — fixture journals under `specify/plugins/change/skills/execute/fixtures/**/journal.yaml*` still carry the old keys and will need regeneration on the next skill-fixture pass; out of scope for this repo. Full `cargo test --all-targets` + `cargo clippy --all-targets -- -D warnings` clean, no regressions.
- **F-04.** Applied. Actual ΔLOC `−55` in `src/commands/slice/merge.rs` (406 → 351) vs predicted `−60`; the 5-LOC gap is the new `'a` lifetime parameter on `RunBody`/`PreviewBody`/`ConflictCheckBody` plus the matching `&'a [MergePreviewEntry]` / `Vec<&'a MergePreviewEntry>` / `&'a [BaselineConflict]` field rewrites that traded three `From` impls for one borrowed envelope each. "Done when" (`rg 'struct (MergedEntry|SpecPreviewEntry|ConflictRow)' src/commands/slice/merge.rs` → nothing) flipped cleanly. Wire change: `slice merge run`'s `merged-specs[]` entries now carry `baseline-path` (matching `slice merge preview`'s spec entries) — `tests/fixtures/e2e/goldens/merge-two-spec.json` updated, golden compares clean after `<TEMPDIR>` substitution; conflict-check wire shape unchanged (`BaselineConflict.defined_at: String` already kebab-rendered, `baseline_modified_at: Timestamp` now flows through `specify_error::serde_rfc3339` — same `%Y-%m-%dT%H:%M:%SZ` strftime as the deleted `From` impl). New helper `crates/error/src/serde_path_display.rs` (24 LOC, mirrors `serde_rfc3339`) with one `#[expect(clippy::ptr_arg)]` because serde's `serialize_with` requires `&PathBuf`, not `&Path`. Full `cargo make ci` clean (clippy `-D warnings`, nextest, doc, vet, deny, fmt), no regressions.
- **F-05.** Applied (in the `specify` repo, not `specify-cli`). Actual ΔLOC `−75` in `.cursor/rules/project.mdc` (310 → 235) vs predicted `−74`; both "Done when" assertions flipped cleanly (`wc -l` → 235 ≤ 240; `rg '^### Frontmatter shape' .cursor/rules/project.mdc` → nothing). One side-effect: `scripts/checks/prose.ts::checkSkillNumericCaps` listed `project.mdc` as a source-of-truth for the `512`/`200` literals; with the duplicated section gone those numbers are no longer in the rule file, so dropped its entry from the FILES table (the check still pins the schema, `docs/standards/skill-authoring.md`, and the two checker scripts). `make checks` clean, no regressions.
- **F-06.** Applied (in the `specify` repo). Actual ΔLOC `−22` in `plugins/change/skills/execute/SKILL.md` (145 → 123) vs predicted `−20`; the 2-LOC gap is the two adjacent guardrail bullets I had to touch to absorb the deleted section's residual rule (line 126 broadened from "supervised single-slice run" to "every `failed`/`blocked` transition (supervised, `loop`, and self-heal alike)"; line 130 swapped "as steps 11c / 12c" for "as the failed/blocked guardrail above" to drop the now-stale numbered references that only made sense inside the deleted at-a-glance list). "Done when" (`rg -c '^## Per-slice algorithm at a glance' plugins/change/skills/execute/SKILL.md` → 0) flipped cleanly. The two `11c`/`12c` references in `per-slice-algorithm.md` (lines 166, 278) are still valid — that file is the full algorithm and owns the numbered steps. `make checks` clean, no regressions; `make test` failure (`tests/cross_repo.ts` missing) is pre-existing and unrelated.
- **F-07.** Applied. Actual ΔLOC `−31` (5 files × 267 LOC → 1 file × 236 LOC) vs predicted `−15`; the review undercounted the dedup — each of the four impl-side files carried its own 3-line module doc header plus a 3-to-4-line `use` block, and `pid.rs`'s `pub(super) fn is_pid_alive` lost its visibility qualifier and one `use super::pid::is_pid_alive` import disappeared from both `acquire` and `status`. "Done when" (`rg --files crates/domain/src/change/plan/lock/ | wc -l` → 1) flipped cleanly. `crates/domain/src/change/plan/lock/tests.rs` kept its `use super::*` — works unchanged since `mod tests;` in `lock.rs` still resolves to `lock/tests.rs`. `cargo build -p specify-domain`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo test --workspace` all clean; the 12 `change::plan::lock::tests::*` test cases pass, no regressions.
- **F-08.** Applied. Actual ΔLOC `−22` in `src/commands/slice/touched.rs` (143 → 121) vs predicted `−20`; offset by `+3` LOC in `crates/domain/src/slice/actions/overlap.rs` (83 → 86: added `use serde::Serialize;`, `Serialize` in the derive list, `#[serde(rename_all = "kebab-case")]`, and the `sort_by` closure widened from one to three lines under `rustfmt` once the field rename `other` → `other_slice` pushed it past the column limit). Net workspace ≈ `−19`. "Done when" (`rg 'struct OverlapRow' src/commands/slice/touched.rs` → nothing) flipped cleanly. Wire shape unchanged: `Overlap.{other,ours,theirs}` → `{other_slice,our_spec_type,other_spec_type}` collapses to the existing kebab keys (`other-slice`, `our-spec-type`, `other-spec-type`) that `tests/slice.rs:261-264` already asserted, and `our_spec_type`/`other_spec_type` flipped from `String` (via `SpecKind::to_string()`) to `SpecKind` directly — the enum's existing `#[serde(rename_all = "kebab-case")]` makes them serialise identically, dropping the two `.to_string()` calls on the hot path as predicted. `OverlapBody` gained an `'a` lifetime so `overlaps: &'a [Overlap]` borrows the action's `Vec<Overlap>` (matching the F-04 pattern). Full `cargo build --workspace --all-targets`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo test --workspace` clean, no regressions.
- **T-01.** Applied. Actual ΔLOC `−14` (`src/commands/change.rs` `−14` of T-01's share + `src/commands/plan/create.rs` `−8` = `−22`) vs predicted `−16`; the small overshoot is the matching `serde::Serialize` import shrink at the bottom of each file once `PathRef` was the only `#[derive]` user removed. "Done when" (`rg 'struct PathRef\b' src/commands` → nothing) flipped cleanly. Wire change: `change create` body's `brief: { path: "..." }` and `plan: { path: "..." }` collapse to bare strings; `tests/fixtures/plan/{init-success,plan-create}.json` regenerated and `tests/change_create.rs::create_json_response` plus `tests/plan_orchestrate.rs::{change_create_empty_json_matches_golden,plan_create_scaffolds_plan_only_json_matches_golden}` now read `actual["plan"].as_str()` instead of `actual["plan"]["path"].as_str()`. No regressions.
- **T-02.** Applied. Actual ΔLOC `−20` in `src/commands/slice/cli.rs` (307 → 287) vs predicted `−17`; the 3-LOC bonus is the deleted `use clap::ValueEnum;` line that was only needed by `CreateIfExistsArg` (`Phase` already pulled clap via the domain side, and `TransitionTarget` keeps its own `clap::ValueEnum` derive). `crates/domain/src/slice/actions/create.rs` grew by `+2` LOC for the new `clap::ValueEnum` derive on `CreateIfExists` and a doc-rephrase, so net workspace `≈ −18`. "Done when" (`rg 'enum CreateIfExistsArg' src/commands/slice/cli.rs` → nothing) flipped cleanly. The `From<CreateIfExistsArg> for CreateIfExists` impl and the `if_exists.into()` call site in `src/commands/slice.rs` are gone — `clap` value-parses directly into the domain enum. No wire change, no regressions.
- **T-03.** Applied. Actual ΔLOC `−24` in `src/commands/slice/lifecycle.rs` (145 → 121) vs predicted `−22`; offset by `+5` LOC in `crates/domain/src/slice/actions/create.rs` for the new `Serialize` derive, the `serde_path_display::serialize` adapter on `dir`, the `#[serde(flatten)]` on `metadata`, and the `use serde::Serialize;` import. Net workspace `−19`. "Done when" (`rg 'struct CreateBody' src/commands/slice/lifecycle.rs` → nothing) flipped cleanly. Wire change: `slice create` body switches from `{name, slice-dir, status, capability, created, restarted}` to `{dir, <SliceMetadata flattened: version, capability, status, created-at, touched-specs, ...>, created, restarted}` — `name` is gone (was synthesised from `dir.file_name()`), `slice-dir` is renamed to `dir`, and the full metadata snapshot is now part of the response. `tests/slice.rs::create_writes_dir_and_metadata` updated to assert `value["dir"].ends_with("/my-slice")` instead of `value["name"] == "my-slice"`. No skill consumer reads beyond `created` (verified across `specify/plugins/`), so the wire widening is safe. No regressions.
- **T-04.** Applied. Actual ΔLOC `−3` in `src/cli.rs` (178 → 175) vs predicted `−11`; the gap is the per-importer `use` lines paid in `src/commands.rs` (`+3` for the canonical paths to `CapabilityAction`/`ToolAction`/`WorkspaceAction`) and `src/commands/{change,codex,compatibility,context,plan,registry}.rs` (each kept its single-line action import, switched from `crate::cli::*Action` to `self::cli::*Action` — wash) plus `src/commands/slice.rs` (`+1` after rearranging the `use cli::{...}` block). Net workspace `≈ +1` LOC, a wash. The 7 cross-module `pub use` re-exports are gone. "Done when" (`rg -c '^pub use crate::commands::' src/cli.rs` → 0) flipped cleanly. Module-edge benefit (operator can grep canonical paths) realised even though LOC stayed flat. No regressions.
- **T-05.** Applied. Actual ΔLOC `−60` in `src/commands/slice/outcome.rs` (264 → 204) vs predicted `−18`; the review undercounted by treating it as "same pattern as F-01/F-03/F-08" but the deleted code also covered `RegistryProposalRow` (10 LOC), its `from_kind` constructor (24 LOC), the `serde_json::Value` widening of `context`, and the bespoke `#[serde(rename = "outcome")]` mirror of the domain's existing rename. `Outcome` from `specify_domain::slice` now serialises directly through `ShowBody<'a> { name, outcome: Option<&'a Outcome> }`. Wire change: the registry-amendment-required variant's payload is no longer hoisted into a sibling `outcome.proposal` object — it now appears as `outcome.outcome.registry-amendment-required.{proposed-name, proposed-url, ...}`, the externally-tagged form already used on disk. `tests/slice.rs::outcome_registry_amendment_writes_payload` updated to read `outcome["outcome"]["registry-amendment-required"]["proposed-name"]` instead of `outcome["proposal"]["proposed-name"]`. The unit-variant wire shape (`outcome: "success"` etc.) is identical to before, so `tests/fixtures/e2e/goldens/slice-outcome.json` and the four other `outcome show` tests pass unchanged. The `context` field flipped from `Value` (Null when absent) to `Option<String>` with `skip_serializing_if = Option::is_none` — `tests/slice.rs::outcome_null_context_when_unstamped` survives because `serde_json::Value::Index` returns `Value::Null` for missing keys (so `.is_null()` is true either way). No regressions.
- **T-06.** Applied. Actual ΔLOC `−9` net (`crates/error/src/display.rs` deleted at `−78`, `crates/error/src/error.rs` grew `+70`, `crates/error/src/lib.rs` shrunk `−1`) vs predicted `−6`. "Done when" (`rg --files crates/error/src/ | wc -l` shows 5) is `6` instead — the gap is `serde_path_display.rs` added in F-04 after the review was authored; the review didn't know about it. Discounting that, the count matches the predicted "5 (or 4 if the same pass swallows yaml.rs)" intent. `Error::hint` and `Error::variant_str` now live in the same `impl Error` block as `validation_failed` in `error.rs`, with `use std::borrow::Cow;` lifted to the file header. The `diag_round_trip` unit test moved into a fresh `mod tests` at the bottom of `error.rs`. `pub mod display;` removed from `lib.rs`. No regressions.
- **T-07.** Applied. Actual ΔLOC `−12` in `src/commands/change.rs` (T-07 share, on top of T-01's `−14`) vs predicted `−10`; the bonus is `brief_show` collapsing the call into an inline closure that captures `&path` directly, eliminating the standalone `write_brief_show_text` helper. "Done when" (`rg 'struct BriefShowBody' src/commands/change.rs` → nothing) flipped cleanly. Wire change: `change show` JSON envelope no longer carries `path` — for the absent case the body is now bare `null` (was `{path: "..."}`), and for the present case it's the bare `ChangeBrief` shape `{frontmatter, body}` (no `path` sibling). `tests/change_show.rs::show_absent` updated to assert `actual.is_null()` and dropped the `path` reader; `show_valid_text_and_json` already only read `frontmatter` / `body` so survives unchanged; the text writer still prints the path on both branches via the captured closure variable, so `change show --format text` is unchanged. No regressions.
- **T-08.** Applied. Actual ΔLOC `−4` in `src/commands/workspace.rs` (304 → 300) vs predicted `−7`; the smaller delta is because `Absent {}` → `Absent` is a 4-char shrink on one line (no LOC), and the 5-line `#[expect(...)]` block collapsed to nothing minus a 1-line blank-keep — net 4 LOC. "Done when" (`rg '#\[expect\(.*empty_enum_variants' src/commands/workspace.rs` → nothing) flipped cleanly. Wire change: `workspace status` now emits `{kind: "absent"}` / `{kind: "present", slots: [...]}` instead of `{}` / `{slots: [...]}`. The `tests/workspace.rs` suite never asserts the absent JSON shape (the absent-registry path is exercised only via `text` format), and the present-path tests still read `value["slots"]` which is unchanged. No regressions.

## Notes on items considered and dropped

- **`crates/error/src/yaml.rs` (14 LOC) → inline `serde_saphyr` errors as two `Error` variants directly.** Dropped: `YamlError` is `pub` and consumed by `crates/tool/src/error.rs:100, 184` (`Box<specify_error::YamlError>`); the deletion would force a parallel rename there for negative LOC gain.
- **`crates/domain/tests/{capability,workspace,finalize,registry}.rs` (1179 / 1042 / 949 / 923 LOC).** Dropped: each holds a distinct integration domain; no concrete duplication evidence surfaced in a 30-min scan. Hardly a tidy.
- **`src/commands.rs` `scoped` vs `dispatch` vs `run_tool` collapse.** Dropped: would require widening `Result<()>` to `Result<Option<u8>>` or similar; cost > saving for ~30 LOC.
- **`change.rs` `MergeOperation` `_ => "UNKNOWN operation"` arms.** Dropped: `MergeOperation` is `#[non_exhaustive]` upstream — catch-alls are load-bearing.
- **`change-execute` SKILL.md "Status." paragraph at line 22.** Soft project-status prose tends to go stale, but it's 1 line and only mildly informational; the duplication finding (F-06) is the higher-value attack.
