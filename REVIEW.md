# Code & Skill Review — single pass, deletion-biased

**Scope.** Both repos: `specify-cli` (Rust) and `specify` (plugins + docs). Pre-1.0; no back-compat.

**Summary.** Top three by LOC removed: (F1) `docs/contributing/skill-authoring.md` (224 LOC) and `docs/contributing/skill-anatomy.md` (194 LOC) duplicate the canonical 78-LOC `docs/standards/skill-authoring.md` for **~−370 LOC**; (F2) the stale 13-crate dependency tree (real shape: 4 lib crates) still ships in `docs/standards/architecture.md` + `docs/release.md` + two test/schema READMEs for **~−95 LOC**; (F3) `## What this skill does NOT do` tables across five `plugins/{spec,change}/skills/*/SKILL.md` files re-paraphrase the 23-LOC `plugins/references/guardrails.md` for **~−60 LOC**, joint-third with (F4) `crates/tool/src/validate.rs::ValidationResult` mirroring `specify_error::{ValidationStatus, ValidationSummary}` for **~−60 LOC**. Total ΔLOC if all ten structural findings land: **~−800 LOC** across the two repos, plus a sharper documentation map. The finding most likely to break in remediation is **F5** — deriving `Serialize` on `MergeOperation` is a wire change pinned by `tests/fixtures/e2e/goldens/`; the `#[serde(tag = "kind", rename_all = "kebab-case")]` shape must reproduce the current JSON byte-for-byte.

## Recon (verified)

### `specify-cli`

| metric | value | source |
|---|---|---|
| `.rs` files (excl. `target/`) | 244 | `Glob '**/*.rs'` |
| `.rs` LOC | 45,235 (tests 7,600 / src 37,635) | `wc -l` aggregate |
| `mod.rs` files | 3 — all under `tests/`, idiomatic 2018-edition test-helper exception | `Glob '**/mod.rs'` |
| `#[test]` count | ≈1,100 across 91 files | `Grep '^\s*#\[test\]'` |
| `crates.io` workspace members | 4 lib crates (`error`, `validate`, `domain`, `tool`) + binary + `xtask`; sibling `wasi-tools/` ws has 2 | `Cargo.toml` `members = […]` |
| largest non-test `src` files | `crates/tool/src/validate.rs` 578, `src/commands/slice/merge.rs` 473, `crates/domain/src/config.rs` 465, `crates/tool/src/manifest.rs` 455 | per-file `wc -l` |
| largest test files | `tests/change_plan_orchestrate.rs` 1904, `tests/slice.rs` 1315, `crates/domain/tests/capability.rs` 1179 | per-file `wc -l` |
| `docs/standards/*.md` LOC | 505 across 5 files | `wc -l` |
| `AGENTS.md` / `DECISIONS.md` | 78 / 284 | `wc -l` |
| `cargo tree --duplicates` | 18 duplicate version pairs (Wasmtime + wasm-pkg surface; covered by `multiple_crate_versions = "allow"` at workspace root) | `DECISIONS.md` §"Follow-up" |
| distinct `Diag` codes | ≈200 across ≈55 files | `Grep 'code:\s*"[a-z]'` |

### `specify` (plugins + docs)

| metric | value | source |
|---|---|---|
| plugin SKILL.md files | 28; biggest `extract` 197, `code-reviewer` 185, `analyze` 168, `define` 159 — all at or under the published 200-line cap | `wc -l` |
| duplicate plugin reference content | 0 hash-collisions (per-skill `references/` are dir-symlinks: `plugins/spec/skills/{init,define,build,merge,drop}/references → ../../references`) | `find … -type l` |
| `docs/` LOC | 7,138 across `tutorials/how-to/reference/explanation/orientation/contributing/standards/appendices` | per-dir `wc -l` |
| `docs/standards/*.md` LOC | 214 across 3 files (`cli-contract` 108, `skill-authoring` 78, `predicates` 28) | `wc -l docs/standards/*.md` |
| `docs/contributing/*.md` LOC | 1,191 across 9 files (`skill-authoring` 224, `checks` 224, `capability-anatomy` 200, `skill-anatomy` 194, `skills-test-coverage` 159, `cli-architecture` 158, `plugin-development` 131, `index` 58, `acceptance` 33) | `wc -l docs/contributing/*.md` |
| `AGENTS.md` LOC | 115 | `wc -l AGENTS.md` |
| `plugins/references/*.md` LOC | 1,235 (`cli-output-shapes` 756, `specify` 257, `agent-teams` 199, `guardrails` 23) | `wc -l plugins/references/*.md` |
| `## What this skill does NOT do` sections | 5 sites (`spec/skills/{define,build,extract}/SKILL.md`, `change/skills/execute/SKILL.md`, `spec/references/init-runbook.md` + symlinked copy via `spec/skills/init/references/init-runbook.md`) | `Grep '## What this skill does NOT do' plugins/` |
| body-cap drift | `docs/standards/skill-authoring.md:31` claims **250**; `scripts/checks/skill_body.ts:24` enforces **200**; `AGENTS.md:79`, `docs/standards/predicates.md:18`, `docs/contributing/skill-authoring.md:107,213`, `docs/explanation/decision-log.md` all say **200** | direct read |
| `checkSkillNumericCaps` `FILES` list | 6 entries; `docs/standards/skill-authoring.md` is **not** among them | `scripts/checks/prose.ts:208-215` |

---

## F1 — Delete `docs/contributing/skill-{anatomy,authoring}.md`; merge into `docs/standards/skill-authoring.md`

- **Repo.** `specify`.
- **Evidence.** Three files describe the same thing:
  - `docs/standards/skill-authoring.md` (78 LOC) — the **normative** version (`AGENTS.md:71` cites it as "the checklist").
  - `docs/contributing/skill-authoring.md` (224 LOC) — re-derives every cap (`:107` "≤ 200 lines", `:213` "≤200 / ≤45"), the description grammar (`:165-180`), the forbidden-frontmatter list, the body-discipline rules.
  - `docs/contributing/skill-anatomy.md` (194 LOC) — re-derives directory shape (`:5-20`), frontmatter field order (`:34-43`), every field definition (`:47+`).
  - `Grep '200|cap|line' docs/contributing/skill-authoring.md` returns 7 hits restating `skill_body.ts:24`.
  - The standards file already names itself "the rules `make checks` enforces" (`:3`); the contributing files duplicate that prose under the guise of "long-form rationale" but only ~50 LOC are rationale — the other ~370 are re-derivation.
  - `checkSkillNumericCaps` (`scripts/checks/prose.ts:208-215`) already maintains a 6-entry sync list across two of these docs; collapsing 3 docs to 1 actively reduces the sync-check surface.
- **LOC.** 78 + 224 + 194 = 496 → ~120 LOC after merge (irreducible rationale: progressive-disclosure paragraph, "why 200 specifically", forbidden-frontmatter list). Net: **−370 LOC**.
- **Action.**
  1. Move the irreducible rationale (`docs/contributing/skill-authoring.md:107-111` "Why a ceiling at all" / "Why 200 specifically"; the forbidden-frontmatter list; any worked-example of good/bad descriptions) into a `## Rationale` H2 appended to `docs/standards/skill-authoring.md`.
  2. Delete `docs/contributing/skill-authoring.md` and `docs/contributing/skill-anatomy.md`.
  3. Update `AGENTS.md:71` and `docs/contributing/index.md` to point only at `docs/standards/skill-authoring.md`. `Grep 'skill-anatomy.md|contributing/skill-authoring.md' docs/ plugins/` and update every caller.
  4. Drop the two paths from `checkSkillNumericCaps`'s `FILES` list (`scripts/checks/prose.ts:211-212`).
- **Done when.** `ls docs/contributing/skill-{anatomy,authoring}.md` returns "no such file"; `make checks` passes; `rg 'skill-anatomy.md|contributing/skill-authoring.md' docs/ plugins/ AGENTS.md` returns 0 hits outside `rfcs/archive/`.
- **Quality delta.** **−370 LOC, −2 docs, −2 cap-sync FILES entries, fewer cross-doc edges.**
- **Rule?** No — one-shot consolidation.
- **Counter-argument.** "Diátaxis says `standards/` and `contributing/` are different audiences." Loses: the 78-LOC standards file already addresses new authors ("Extend the allow-list when a new verb is genuinely imperative" — `:11`). Helix and jj keep one CONTRIBUTING.md; cargo's `src/doc/contrib/` is many pages but each is a distinct topic.
- **Depends on.** None.

---

## F2 — Collapse the stale 13-crate dependency tree (specify-cli)

- **Repo.** `specify-cli`.
- **Evidence.** Workspace = 4 lib crates (`crates/{error,validate,domain,tool}`) + binary + `xtask` (`Cargo.toml:50-56`); `DECISIONS.md:123-156` confirms the Phase-1B collapse. The pre-collapse tree still ships in:
  - `docs/standards/architecture.md:9-23` — 15-line ASCII tree naming `specify-{capability,spec,task,slice,merge,config,validate,change,init}` (every line wrong).
  - `docs/standards/architecture.md:50` — "the `Layout<'a>` newtype in `specify-config` (`crates/config/src/lib.rs`)" — path no longer exists (`Layout` is at `crates/domain/src/config.rs`).
  - `docs/release.md:36-40` — publish order names retired crates; `.github/workflows/release.yaml` actually publishes `error → validate → domain → specify`.
  - `schemas/plan/README.md:14` — "performed by `Plan::validate` in `specify-change`".
  - `tests/fixtures/parity/README.md:3,9,14,18` — references `specify-spec` / `specify-merge` / `specify-validate` as Rust crates.
  - `crates/tool/src/validate.rs:27` — doc-comment "`specify-tool` does not depend on `specify-capability`" (the crate is gone).
  - `crates/domain/src/init/git.rs:15` — tempdir label `specify-capability-checkout` leaks the dead crate name.
  - Current LOC at these sites: ~55 LOC of prose + the 15-line tree → ~30 LOC after rewrite. Net: **−95 LOC**.
- **Action.**
  1. Replace `architecture.md:9-23` with the 6-line tree at `DECISIONS.md:123-148` (or link to it); fix the `specify-config` path at `architecture.md:50`.
  2. Rewrite `docs/release.md:36-40` to mirror the publish order in `.github/workflows/release.yaml` (`error → validate → domain → specify`).
  3. In `schemas/plan/README.md:14` drop the crate name — just say "the CLI".
  4. In `tests/fixtures/parity/README.md` swap the three crate names for module paths (`specify_domain::merge`, `specify_validate::validate_baseline_contracts`).
  5. Delete `crates/tool/src/validate.rs:26-27` doc-comment about `specify_capability`; rename the tempdir prefix at `init/git.rs:15` to `specify-checkout`.
- **Done when.** `rg -n 'specify-(capability|spec|task|change|merge|init|registry|slice|config)\b' AGENTS.md docs/ schemas/ tests/fixtures/parity/README.md crates/tool/src/validate.rs crates/domain/src/init/git.rs` returns 0 hits. (The match in `crates/domain/src/merge.rs:9` is an explicit `module_inception` waiver comment and is allowed to keep its archaeology — it's annotated.)
- **Quality delta.** **−95 LOC; reality-aligned crate map.**
- **Rule?** No. The drift exists once; a clippy lint cannot police prose.
- **Counter-argument.** "Old names help readers tracing pre-collapse PRs." Loses: `DECISIONS.md §Crate layout` preserves the history note in one place.
- **Depends on.** None.

---

## F3 — Delete the five `## What this skill does NOT do` tables; survivors merge into `## Guardrails`

- **Repo.** `specify`.
- **Evidence.** Five sites carry tables that paraphrase `plugins/references/guardrails.md` (23 LOC, canonical):
  - `plugins/spec/skills/define/SKILL.md:134-145` — 7 rows. Rows 3 (metadata writes), 4 (plan status), 5 (plan entries) restate `guardrails.md §Single-writer`.
  - `plugins/spec/skills/build/SKILL.md:88-99` — 7 rows. Rows 3 (metadata), 4 (plan status), 5 (baseline merge) restate `guardrails.md`.
  - `plugins/spec/skills/extract/SKILL.md:153-164` — 8 rows. Rows 2 (slice-dir scope), 4 (baseline merge), 5 (transition status) restate `guardrails.md`.
  - `plugins/change/skills/execute/SKILL.md:117-128` — 9 rows. Rows 1, 2, 3, 4 (`plan.yaml` entries, status, metadata, journal) **all** restate `guardrails.md §Single-writer`.
  - `plugins/spec/references/init-runbook.md:175-185` (+ symlinked copy via `init/references/init-runbook.md`) — 7 rows.
  - `AGENTS.md:81` and `docs/standards/skill-authoring.md:53` both say "SKILL.md files **link** to them; they do **not** restate them inline" — these tables violate the rule the project authored for itself. `checkOneGuardrailsBlockPerSkill` (`scripts/checks/skill_body.ts`) inspects `## Guardrails` blocks; the parallel `## What this skill does NOT do` H2 dodges it.
- **LOC.** ~13 rows redundant × 2 LOC ≈ 26 LOC of pure restatement. Per-file table frame (header + dashes + blank, 3 LOC) × 5 files = 15 LOC. Skill-specific surviving rows collapse into the existing `## Guardrails` H2. `extract` (197 LOC) ratchets to ~183, leaving room for the next addition. Net: **−55 to −70 LOC**.
- **Action.**
  1. In each of the five files, delete rows whose Surface is `.metadata.yaml` / `plan.yaml` (status or entries) / `journal.yaml` / archive moves / "Write outside slice-dir" / "Transition slice status" / "Merge into baseline" — all in `plugins/references/guardrails.md §Single-writer` or `§Baseline immutability`.
  2. Rewrite surviving skill-specific rows as bullets under the existing `## Guardrails` H2. If the table empties (likely for `execute` — rows 6-9 are "Yes — see [foo.md]" wire pointers, not don'ts), delete the H2 entirely.
  3. Replace each deleted H2 with one sentence ending in `> See [plugins/references/guardrails.md](../../../references/guardrails.md) for the shared single-writer rules.`
- **Done when.** `rg -c '^## What this skill does NOT do' plugins/` returns ≤ 1 (ideally 0). `make checks` still passes; `checkOneGuardrailsBlockPerSkill` does not regress because surviving rows append into the existing `## Guardrails` block.
- **Quality delta.** **−60 LOC, −5 H2 boundaries, −13 cross-file restatements of the single-writer rule.**
- **Rule?** No — `checkOneGuardrailsBlockPerSkill` is already the rule; this finding aligns reality with it.
- **Counter-argument.** "The `Surface | Status` table conveys 'forbidden surface' visually better than a bullet list." Loses: the surfaces are the same surfaces in every skill, the rule lives in `guardrails.md`, and the visual emphasis is around prose the agent re-reads on every invocation — the exact attention-cost the 200-line body cap exists to defend.
- **Depends on.** None.

---

## F4 — Collapse `specify_tool::validate::ValidationResult` into `specify_error::ValidationSummary`

- **Repo.** `specify-cli`.
- **Evidence.** `crates/tool/src/validate.rs:30-68` defines a `ValidationResult { Pass, Fail, Deferred }` enum that mirrors `specify_error::{ValidationStatus, ValidationSummary}` (`crates/error/src/validation.rs:6-37`); the doc-comment at `:26-27` says it "mirrors `specify_capability::ValidationResult`" — a crate that no longer exists. `Deferred` is **never constructed** (`Grep 'ValidationResult::Deferred|Deferred {' crates/tool` matches only the variant declaration and the `rule_id()` arm). The only external consumer is `src/commands/tool.rs::validation_failure`, which always discards `Pass` and translates `Fail → ValidationSummary`. Three test sites (`crates/tool/src/validate.rs:485, 494, 518`) just assert "all results are `Pass`" — they only need a boolean.
- **LOC.** Lines 24-68 (45) + the `pass`/`fail` constructors at 103-113 (11) + `validation_failure` collapses from 14 to 5 + the test assertion-helper goes from 9 to 2. Current ≈ 80; proposed ≈ 20. Net: **−60 LOC**.
- **Action.**
  1. Change `Tool::validate_structure` and `ToolManifest::validate_structure` to return `Vec<specify_error::ValidationSummary>`.
  2. Inline `pass(rule_id, rule)` as `ValidationSummary { status: ValidationStatus::Pass, rule_id: rule_id.into(), rule: rule.into(), detail: None }`; `fail` similarly with `Some(detail)`.
  3. Rewrite `src/commands/tool.rs::validation_failure` as `summary.status == ValidationStatus::Fail`.
  4. Update the three "all `Pass`" tests to `assert!(results.iter().all(|s| s.status == ValidationStatus::Pass))`.
  5. Delete `crates/tool/src/validate.rs:26-27` (subset of F2 here).
- **Done when.** `rg -n 'enum ValidationResult' crates/tool/` returns nothing; `rg -n 'Deferred' crates/tool/` returns nothing.
- **Quality delta.** **−60 LOC, −1 type, −1 branch (`Deferred`), −1 module edge.**
- **Rule?** No.
- **Counter-argument.** "The `Deferred` variant is wire-contract surface across the workspace." Loses: `specify_tool::validate` never reaches the wire — only its `Fail` summaries do via `Error::Validation`, and `ValidationStatus::Deferred` already exists upstream for the day a tool rule needs it.
- **Depends on.** None.

---

## F5 — Derive `Serialize` on `MergeOperation`; delete the `MergeOp` mirror

- **Repo.** `specify-cli`.
- **Evidence.** `src/commands/slice/merge.rs:252-316` defines `enum MergeOp { Added, Modified, Removed, Renamed, CreatedBaseline, Unknown }` plus a 33-line `impl From<&specify_domain::merge::MergeOperation> for MergeOp`. The mirror exists solely so `MergeOp` can carry `#[derive(Serialize)] #[serde(tag = "kind", rename_all = "kebab-case")]`. `MergeOperation` (defined `crates/domain/src/merge/merge.rs:31-68`) is the only domain enum that hand-rolls a wire mirror — every other operation enum either derives `Serialize` directly (e.g. `OpaqueAction` in `crates/domain/src/merge/slice/`) or sidesteps it via free-function rendering. The wire variant `Unknown` is dead: every domain variant maps explicitly; the `_ => Self::Unknown` arm exists for the `#[non_exhaustive]` future-proof which a `serde(other)` annotation handles for free. Style guidance is on side: `docs/standards/style.md:34-44` — "One body per command, no wrapper newtype". Cargo's wire types follow the same idiom — `derive(Serialize)` on the domain enum, not on a CLI mirror.
- **LOC.** 65 → 10 (six-line `#[derive(Serialize)]` + `#[serde(tag = "kind", rename_all = "kebab-case")]` block at the domain site, plus four-line replacements for `operations: Vec<MergeOp>` → `Vec<MergeOperation>`). Net: **−55 LOC**.
- **Action.**
  1. On `crates/domain/src/merge/merge.rs:30-68`, add `#[derive(Serialize)]` and `#[serde(tag = "kind", rename_all = "kebab-case")]` (`serde.workspace = true` already declared on `crates/domain/Cargo.toml`).
  2. Delete `src/commands/slice/merge.rs:251-316` (`enum MergeOp`, `From<&MergeOperation>`, `operation_label`, the `Unknown` variant). Replace `Vec<MergeOp>` on the three `*Body` structs with `Vec<MergeOperation>`. Rewrite `summarise_ops` over `MergeOperation` directly.
  3. Regenerate `tests/fixtures/e2e/goldens/` under `REGENERATE_GOLDENS=1`; verify `git diff` is empty (on-disk shape is the constraint — `kind` discriminant + kebab names already match).
- **Done when.** `rg -n 'enum MergeOp\b' src/` returns nothing. `REGENERATE_GOLDENS=1 cargo nextest run --test slice_merge` produces an empty `git diff` against the goldens.
- **Quality delta.** **−55 LOC, −1 type, −1 branch (`Unknown`), −1 module edge, −1 From-impl.**
- **Rule?** No. The pattern only exists once in `src/commands/`; if a second `*Op` mirror appears later, then a rule.
- **Counter-argument.** "Wire DTOs should be a hard boundary between domain and CLI." Loses: the domain → wire shape is already locked-in (`kind`-tagged, kebab-case); the mirror does not add a degree of freedom the test goldens are not already pinning, and `#[non_exhaustive]` survives serde via a `#[serde(other)]` catch-all on the consumer side.
- **Depends on.** None.

---

## F6 — Drop the resolved "wasm-pkg-client follow-up" from DECISIONS.md

- **Repo.** `specify-cli`.
- **Evidence.** `DECISIONS.md:234-284` is a 51-line "Follow-up" reasoning through three options for gating `wasm-pkg-client`. `crates/tool/Cargo.toml:26` already ships option (1): `oci = ["dep:wasm-pkg-client", "dep:tokio", "dep:futures-util"]`, and the binary's `Cargo.toml:17` sets `default = []` (OCI off by default). The text describes wasm-pkg-client as "wired in as a non-optional dep" — false today.
- **LOC.** Net: **−51 LOC** in DECISIONS.md.
- **Action.**
  1. Delete `DECISIONS.md:234-284` ("## Follow-up: wasm-pkg-client HTTP duplication").
  2. Keep the existing one-liner in `crates/tool/Cargo.toml:22-26` (already says what the feature does); no further prose required.
- **Done when.** `wc -l DECISIONS.md` returns ≤ 234 and `rg 'Follow-up' DECISIONS.md` finds nothing.
- **Quality delta.** **−51 LOC; deletes a finished TODO.**
- **Rule?** No.
- **Counter-argument.** "Future readers want to see why option (1) won." Loses: the implementation is the answer. `git log -p crates/tool/Cargo.toml` carries the history.
- **Depends on.** None.

---

## F7 — Unify `ScaffoldError` and `VectisError` in `wasi-tools/vectis`

- **Repo.** `specify-cli`.
- **Evidence.** Two near-identical error enums live in the same crate:
  - `wasi-tools/vectis/src/validate.rs:80-94` — `VectisError { InvalidProject{message}, Internal{message} }` + `to_json` + `variant_str` + `exit_code` returning `EXIT_FAILURE` (2).
  - `wasi-tools/vectis/src/scaffold/error.rs:10-61` — `ScaffoldError { Io(#[from] io::Error), InvalidProject{message}, Internal{message} }` + identical `to_json` + `variant_str` + `exit_code` returning `1`.
  Apart from `ScaffoldError::Io` and the constant return of `exit_code`, the two are character-for-character duplicates. Both wire-payload shapes are byte-identical (`{"error": "...", "message": "..."}`); only the integer exit code differs. The `EXIT_CLEAN`/`EXIT_FINDINGS`/`EXIT_FAILURE` constants at `validate.rs:16-22` are also redundant — `0`/`1`/`2` literals appear once each in the binary, and integration tests don't reference them (`rg EXIT_FINDINGS wasi-tools/vectis/tests/` is empty).
- **LOC.** 32 (scaffold) + 53 (validate) + 7 (EXIT_* constants) → ≈ 50 unified into one enum. Net: **−40 LOC**.
- **Action.**
  1. In `wasi-tools/vectis/src/lib.rs`, define one `pub enum VectisError { Io(#[from] io::Error), InvalidProject{message: String}, Internal{message: String} }` plus the merged `to_json` and `variant_str`. Replace `exit_code` with a single `pub const EXIT_FAILURE: u8 = 2;` and let `scaffold::render_json` return `0` for success / `2` for failure.
  2. Delete `wasi-tools/vectis/src/scaffold/error.rs` entirely; re-export from `scaffold::error` if any test consumer needs the name.
  3. Drop the per-validate `EXIT_CLEAN`/`EXIT_FINDINGS`/`EXIT_FAILURE` constants; inline `0`/`1`/`2` at the two call sites in `validate.rs` and update the three internal tests to literals.
- **Done when.** `rg 'pub enum (Scaffold|Vectis)Error' wasi-tools/vectis/` returns one hit. `rg 'EXIT_(CLEAN|FINDINGS)' wasi-tools/vectis/` returns nothing.
- **Quality delta.** **−40 LOC, −1 type, −1 module edge, −3 dead constants.**
- **Rule?** No. Single duplication, no third site looming.
- **Counter-argument.** "The two subcommands ship different historical exit-code shapes; merging them is a wire change." Loses: scaffold-failure exit `1` vs validate-failure exit `2` is a distinction without a documented contract; collapsing both to `2` matches the host CLI's typed-error slot. Pre-1.0.
- **Depends on.** None.

---

## F8 — Collapse `codex.rs::{RuleSummary, RuleExport, provenance_text, export_provenance_text}` into one view

- **Repo.** `specify-cli`.
- **Evidence.** `src/commands/codex.rs:167-192` defines two structs that differ only in `trigger`/`body`: `RuleSummary` is `RuleExport` minus two fields. Each has a separate `From<&ResolvedCodexRule>` impl (`:195-228`), and the file then declares two near-identical helpers — `provenance_text` (`:261-272`) and `export_provenance_text` (`:273-283`) — that do the same `match rule.provenance_kind` over the same Option fields. Net duplicate surface: two structs + two `From` impls + two provenance helpers + one `ProvenanceFields` plumbing struct (`:231-259`) that only exists to feed the duplicate `From` impls.
- **LOC.** ≈85 → ≈45 if collapsed to one `RuleView` carrying `Option<&'a str>` for `trigger`/`body` plus one `From` impl plus one `provenance_text`. Net: **−40 LOC**.
- **Action.**
  1. Replace `RuleSummary` and `RuleExport` with a single `#[derive(Serialize)] struct RuleView<'a> { …, #[serde(skip_serializing_if = "Option::is_none")] trigger: Option<&'a str>, #[serde(skip_serializing_if = "Option::is_none")] body: Option<&'a str> }`.
  2. `list` constructs `RuleView { trigger: None, body: None, .. }`; `show` and `export` populate both.
  3. Delete `export_provenance_text`; `provenance_text(&RuleView)` covers both call sites.
  4. Drop the `ProvenanceFields` helper — the single `From` impl reads `resolved.provenance` directly.
- **Done when.** `rg 'struct Rule(Summary|Export)\b' src/commands/codex.rs` returns nothing. `rg 'export_provenance_text' src/commands/codex.rs` returns nothing.
- **Quality delta.** **−40 LOC, −1 type, −1 plumbing struct, −1 helper function.**
- **Rule?** No.
- **Counter-argument.** "`list` and `export` JSON outputs are wire-stable shapes — adding `trigger`/`body` as nullable fields is a wire change." Loses: `skip_serializing_if = Option::is_none` keeps the `list` output byte-identical; `show` and `export` already populate both fields.
- **Depends on.** None.

---

## F9 — Collapse `AGENTS.md` §"Markdown style"–§"Mechanical enforcement" (lines 64-105) into pointers

- **Repo.** `specify`.
- **Evidence.** `AGENTS.md:64-105` (42 LOC) reproduces:
  - The description grammar (`:75`) — already canonical at `docs/standards/skill-authoring.md:7-15` (which `AGENTS.md:71` itself links).
  - The argument-hint grammar (`:77`) — already at `docs/standards/skill-authoring.md:17-27`.
  - Body caps 200/45/512 (`:79`) — sync-checked across 6 sites by `checkSkillNumericCaps` (`scripts/checks/prose.ts:205-232`).
  - Skill-body discipline (`:81-85`) — already at `docs/standards/skill-authoring.md:42-49`.
  - The §"Mechanical enforcement" predicate table (`:87-105`) — re-derives the predicate table at `docs/standards/predicates.md:15-25`. Eight rows × ~1 LOC each ≈ 10 LOC.
  - `AGENTS.md` already routes via "see [docs/contributing/skill-authoring.md] / [docs/standards/skill-authoring.md] / [.cursor/rules/project.mdc]" at `:71`. Once you're a click away, restating the rules in line is the duplication.
- **LOC.** 42 → ~6 LOC of pointers. Net: **−36 LOC**.
- **Action.**
  1. Replace `AGENTS.md:64-105` with a §"Skill authoring" stanza: "Skill authoring rules (description grammar, argument-hint grammar, 200/45/512 caps, body discipline, predicate table) live in [docs/standards/skill-authoring.md](docs/standards/skill-authoring.md) (after F1 consolidation) and [docs/standards/predicates.md](docs/standards/predicates.md). Enforced by `make checks`."
  2. Drop the predicate table here (canonical at `docs/standards/predicates.md`).
  3. Keep §"Cursor Cloud specific instructions", §"Vocabulary", §"Workflow overview", §"Skill / CLI responsibility split", §"Contract skills", §"Plan-driven loop", §"Commands", §"Gotchas", §"Related coding standards" — those carry unique content.
- **Done when.** `wc -l AGENTS.md` returns ≤ 80. `rg 'checkBodyAndSectionLineCounts|checkArgumentHintGrammar|IMPERATIVE_VERBS' AGENTS.md` returns 0 hits. `make checks` passes.
- **Quality delta.** **−36 LOC, −1 predicate-table copy, fewer cap-sync FILES sources (no new entry needed).**
- **Rule?** No — single root file.
- **Counter-argument.** "AGENTS.md is what Cursor reads first; inlining the rules ensures the agent sees them on every session." Loses: `AGENTS.md:71` already points at the canonical files and Cursor follows links; if it didn't, the existing routing in `:71` would already be broken.
- **Depends on.** F1 (do that first so `AGENTS.md` points to a single survivor).

---

## F10 — Drop `docs/standards/predicates.md` once F1 + F9 land

- **Repo.** `specify`.
- **Evidence.** `docs/standards/predicates.md` (28 LOC) is a 9-row table mapping predicate name → script path → allowlist behaviour. Three of the rows are identical wording to the function comments in `scripts/checks/{skill_body,skill_frontmatter,prose}.ts`. After F9, `AGENTS.md` no longer carries the same table; after F1, the canonical `docs/standards/skill-authoring.md` body names every predicate it cares about (`:15, 27, 34, 53`). With F1 + F9, this becomes the third copy of a single-source table.
- **LOC.** 28 → 0. Net: **−28 LOC**.
- **Action.**
  1. Delete `docs/standards/predicates.md`.
  2. In `docs/standards/skill-authoring.md`, replace "See [predicates.md](predicates.md)" (`:15`) with "See [scripts/checks/](../../scripts/checks/) for the implementation."
  3. Remove the cross-link in `docs/contributing/checks.md` and elsewhere (`rg 'predicates.md' docs/`).
- **Done when.** `ls docs/standards/predicates.md` returns "no such file". `rg 'predicates.md' docs/ AGENTS.md` returns 0 hits.
- **Quality delta.** **−28 LOC, −1 doc, −1 cross-doc edge, +0 third copy.**
- **Rule?** No.
- **Counter-argument.** "Operators read prose tables, not TypeScript." Loses: the predicate table tells operators which check failed and where the code lives — that's a CI-output concern; `make checks` already prints the predicate name on failure.
- **Depends on.** F1, F9.

---

## One-touch tidies (≤ 30 LOC each or single-axis, ranked)

1. **Collapse `From<ToolError> for specify_error::Error` to a code-only match** (`-25 LOC`, `specify-cli`). `crates/tool/src/error.rs:254-291` (37 LOC) builds five named `Diag` variants that re-stringify the same content already in `#[error("…")]`. Replace with one `let code = match &err { ToolError::ToolNotDeclared { .. } => "tool-not-declared", … }; Self::Diag { code, detail: err.to_string() }` (12 LOC). Done when `rg -nA3 'impl From<ToolError>' crates/tool/src/error.rs | wc -l` < 18.

2. **Inline the dispatch sub-bullets in `plugins/contract/README.md:36-57`** (`-25 LOC`, `specify`). `:36-46` ("Each skill's `SKILL.md` dispatches to format-specific `author.md`, `importer.md`, and `verifier.md`" + "### Mixed-format ordering") restates content the format SKILL.md files own and the contracts capability build brief already enforces. `:48-57` ("### Cross-project compatibility classification (RM-04)") is a retired-heuristic explainer; the live CLI link is one sentence away at `docs/reference/cli/compatibility.md`. Collapse the dispatch + ordering paragraph to one sentence; delete the RM-04 H3 entirely.

3. **Delete the fictional `Out` / `Render` / `serialize_path` API from the standards docs** (`-18 LOC`, `specify-cli`). `docs/standards/coding-standards.md:87,97,100,106,116,154,169` and `docs/standards/handler-shape.md:17,30,32,60` describe `ctx.out().write(&Body)?`, `Out::for_format(format).write(&Body)`, the `Render::render_text` trait, and a `serialize_path` helper as the canonical handler API. **None exist.** The actual API is `ctx.write(&body, render_text_closure)` (`src/context.rs:82-86`) and free `output::write(format, &body, closure)` (`src/output.rs:31-35`). Rewrite the §Format dispatch / §One emit path / handler-shape stanzas to describe the real API; drop the `serialize_path` field-allowlist row. Done when `rg -n 'ctx\.out\(\)|Out::for_format|trait Render|serialize_path' docs/ src/ crates/` returns nothing.

4. **Delete `ToolError::cache_io` / `source_io` named constructors** (`-15 LOC`, `specify-cli`). `crates/tool/src/error.rs:184-208`; both build the same `Self::Io` variant with three internal call sites total. Replace with `Self::Io { action, path: path.into(), source }` at the call site. `style.md:113` already says "Named constructors are reserved for multi-arg or fallible builders".

5. **Drop the `CommandOutcome::Success(Value)` single-variant enum in `wasi-tools/vectis`** (`-15 LOC`, `specify-cli`). `wasi-tools/vectis/src/validate.rs:65-70` declares `pub enum CommandOutcome { Success(Value) }` — a single-variant enum, explicitly called out by `coding-standards.md:79` as "dead overhead". Every per-mode handler (`tokens`, `assets`, `layout`, `composition`, `all` in `wasi-tools/vectis/src/validate/engine/*.rs`) wraps `Ok(CommandOutcome::Success(json!({…})))`; the re-entry path at `engine.rs:74` immediately destructures. Change signatures to `Result<Value, VectisError>`; inline destructure; delete the enum and the test-only `extract_envelope`. Done when `rg 'CommandOutcome' wasi-tools/` returns nothing.

6. **Drop the Python-history paragraph in `tests/fixtures/parity/README.md:18`** (`-12 LOC`, `specify-cli`). The fixtures are now a Rust-only regression baseline; the `re.MULTILINE` parity-quirk explanation belongs as a one-line code comment at `validate_baseline_contracts`, not next to data files. The README itself says "the Python script has since been retired."

7. **Replace hand-rolled `Display for Status` with `strum::Display`** (`-9 LOC`, `specify-cli`). `crates/error/src/validation.rs:15-23` hand-rolls `Display` for `Status { Pass, Fail, Deferred }` mirroring `#[serde(rename_all = "kebab-case")]`. `style.md:60-69` cites this exact pattern as the canonical anti-example, and `strum 0.28` is already a workspace dep. Add `strum.workspace = true` to `crates/error/Cargo.toml` (one-line); `derive(strum::Display)` + `#[strum(serialize_all = "kebab-case")]`; delete lines 15-23.

8. **Remove `Exit::Code` doc-comment paraphrase at three sites** (`-9 LOC`, `specify-cli`). `src/output.rs:47-53`, `src/commands.rs:117-121`, and `AGENTS.md:31` all restate "`Exit::Code(u8)` is reserved for `specify tool run` WASI passthrough." `DECISIONS.md:21-22` is canonical; collapse the others to one-line references.

9. **Drop the redundant `multiple_crate_versions` waiver in `crates/tool/src/lib.rs:5-11`** (`-7 LOC`, `specify-cli`). Workspace `Cargo.toml:104` already sets `multiple_crate_versions = "allow"`; the cfg-gated crate-level `#![cfg_attr(any(feature = "host", feature = "oci"), allow(...))]` is dead. Done when `rg 'cfg_attr.*multiple_crate_versions' crates/tool/` returns nothing and `cargo make lint` still passes.

10. **Repair body/section caps drift in `docs/standards/skill-authoring.md`** (`0 LOC, −2 fictional predicates`, `specify`). `:31-34` claims "≤ **250 lines** (`checkBodyLineCount`)" and "≤ **60 lines** (`checkSectionLineCount`)". Reality (`scripts/checks/skill_body.ts:24,28`): **200 / 45**, one predicate `checkBodyAndSectionLineCounts`. Worse, `checkSkillNumericCaps`'s `FILES` list (`scripts/checks/prose.ts:208-215`) does **not** include `docs/standards/skill-authoring.md`, so the cap-sync predicate is blind to the only file that has drifted. Replace `250` → `200`, `60` → `45`, drop the two non-existent predicate names, and add `["docs/standards/skill-authoring.md", true, true],` to `prose.ts:215`. Done when `rg '\b(250|60)\b' docs/standards/skill-authoring.md` returns 0 hits and `make checks` passes. (Tidy because LOC is flat, but corrects a wire-relevant policy doc and closes a sync-check blind spot.)

The rest (DTO `String`-vs-`PathBuf` policy in coding-standards.md, the `mod.rs` rule already aligns with reality, doctor.rs `code: "registry-shape".to_string()` cluster, `extract` SKILL.md §"Reference Documentation"+§"Examples" rehash, `extract` `## Guardrails > ### NEVER`/`### ALWAYS` lists) cost more than 200 LOC of churn for under-200 LOC of delete; dropped per the master rule.

---

## Findings dropped during pass

- "Add a `<200 LOC each` clippy rule for new `*Body` types" — adds machinery; violates "do NOT propose".
- "Convert all 200+ `Diag` codes to typed `Error` variants" — adds enormous LOC; the `Diag`-first policy in `DECISIONS.md:52-67` is correct and active.
- "Replace `serde-saphyr` with `serde_yaml_ng`" — `DECISIONS.md §YAML library` is decisive; mass substitution is not deletion.
- "Reduce 1904-line `tests/change_plan_orchestrate.rs`" — `docs/standards/testing.md:13-15` is explicit that the per-file integration target is the chosen shape; no net deletion without losing coverage.
- "Inline `crates/error/src/lib.rs::is_kebab` at the call sites" — investigated and dropped. `is_kebab` is called from **7** production sites; inlining grows the workspace, not shrinks it.
- "Deduplicate `plugins/omnia/references/` against `plugins/omnia/skills/*/references/`" — investigated and dropped. The per-skill "copies" are symlinks (`lrwxr-xr-x ... -> ../../../references/agent-teams.md`); a sha1 walk excluding symlinks finds only 2 trivial fixture-MD dupes worth 84 LOC.
- "Symlink-dedupe `plugins/spec/skills/*/references/` against `plugins/spec/references/`" — already done; the per-skill `references/` are directory symlinks (`find … -type l` confirms 5 dir-symlinks under `plugins/spec/skills/`).
- "Reduce `plugins/references/cli-output-shapes.md` (756 LOC)" — `AGENTS.md:83` says it is regenerated by `make doc-envelopes` from CLI test fixtures. Editing it by hand violates the source-of-truth rule.
- "Collapse `docs/tutorials/cross-repo-{change,execute}.md` + `landing-a-change.md`" — operator-facing walkthroughs with different lockstep entry points. Investigated; net deletion < 30 LOC without breaking links from `docs/SUMMARY.md`.
- "Add a Deno predicate to forbid `## What this skill does NOT do` tables" — adds machinery; the 200-line body cap already creates the right pressure once F3 lands, and `checkOneGuardrailsBlockPerSkill` already covers the spirit. Master rule forbids new predicates without a 3× recurrence after the pass.

---

## Landing order (recommended)

`specify-cli` and `specify` findings are independent and can land in parallel. Within each repo:

- **`specify`**: F1 → F9 → F10 → F3 → tidy #10 (cap drift). F1 enables F9; F9 enables F10. F3 and the cap-drift tidy are independent of the consolidation chain.
- **`specify-cli`**: F6 (DECISIONS-only, zero code-risk) → F2 → F4 → F8 → F5 → F7 → tidies #1, #3, #4, #5, #6, #7, #8, #9 in descending LOC.

Net at the end of the pass: **~−800 LOC across the two repos**, **−6 duplicate-or-fictional types**, **−5 H2 boundaries**, **−4 dead docs (+ ~370 LOC of policy doc compressed)**, **−1 sync-check blind spot**.

---

## Post-mortem

One line per applied finding. Format: `id. actual ΔLOC vs predicted | done-when | regressions`.

- F1. **−380 LOC vs −370 predicted (103%)** | all three done-when assertions flipped cleanly (`ls docs/contributing/skill-{anatomy,authoring}.md` → no such file; `make checks` green; `rg 'skill-anatomy.md|contributing/skill-authoring.md' docs/ plugins/ AGENTS.md` returns 0 hits) | no regressions; appended `## Rationale` H2 (49 lines insert) tripped `checkNoRfcCitationsInDocs` and the layer-number predicate on first run (3× `RFC-N` mentions + one demo `Layer 4` line carried over verbatim from `contributing/skill-authoring.md`) — stripped the RFC literals and rewrote the bad-description demo to use an `§3B writer-protocol` placeholder; second run green. Caller-edit churn: 9 files touched, 51 insertions / 431 deletions; `prose.ts` `FILES` list reduced 6 → 4 entries (sync-check surface −33%). Also collaterally fixed the 250→200 / 60→45 body-cap drift in `docs/standards/skill-authoring.md:31-34` because keeping the old numbers next to the new "Why 200 specifically" rationale paragraph would have left the canonical surface self-contradicting (tidy #10's number-fix half, leaving the fictional-predicate-name half for that tidy). Prior-session prior (delete-heavy F1 hit 98%) was a good fit; the small overshoot came from the 50-line rationale budget compressing to 41 useful lines after RFC-strip + duplicated forbidden-keys content collapsed into the existing standards body's "Forbidden frontmatter" section.
- F2. **−10 LOC vs −95 predicted (11%)** | done-when assertion flipped cleanly (`rg -n 'specify-(capability|spec|task|change|merge|init|registry|slice|config)\b' AGENTS.md docs/ schemas/ tests/fixtures/parity/README.md crates/tool/src/validate.rs crates/domain/src/init/git.rs` returns 0 hits) | no regressions: `cargo check --workspace --all-features` clean, `cargo clippy -Dwarnings` clean, `cargo nextest -p specify-tool` 48/48, `cargo nextest -p specify-domain` 466/466, doc tests green. Touched 9 files (19 insertions / 29 deletions). The big LOC undershoot came from the review counting wholesale prose-block deletions when ~80% of the actual rot was single-token swaps (`specify-config` → `specify-domain`, `specify-merge::validate_baseline` → `specify_domain::merge::validate_baseline`) that net to ±0 LOC per site. Only the architecture.md crate-tree block (13 → 5 lines) and `validate.rs:26-27` doc-comment (−2 lines) delivered net deletions; the rest was correction, not deletion. The review's action list also under-scoped its own done-when grep: `AGENTS.md:9-16` carried the identical stale 6-line crate graph, `docs/standards/handler-shape.md:7` still pointed at `specify-config`, and the BAD example at `docs/standards/style.md:77` literally named `specify-init` — all three would have failed the done-when assertion. Fixed in the same pass (rewrote AGENTS.md's crate graph to match architecture.md's new 5-line tree; swapped the two handler-shape/style references). Prior post-mortem (F1) under-counted exactly the same way — predictions that mix "delete the prose" with "fix the names" tend to bias toward deletion-magnitude when in-place edits dominate; calibration note for future findings tagged "stale crate names" / "stale type names": cap predicted ΔLOC at the literal line-count of the structural blocks being removed, not the count of *sites touched*.
- F3. **−38 LOC vs −60 predicted (63%)** | done-when assertion flipped cleanly and beat the target (`rg -c '^## What this skill does NOT do' plugins/` returns 0 hits — target was ≤ 1, ideally 0; `make checks` green) | no regressions; `define`/`build`/`extract` keep their existing `## Guardrails` blocks (no second H2 introduced, so the implicit "one Guardrails block per skill" invariant holds), `extract`'s already-grandfathered `sectionLineCount = 1` baseline did not need raising, and the `init/SKILL.md` callers that referenced "the 'what this skill does NOT do' matrix" were updated in the same pass. Touched 6 files (31 insertions / 68 deletions). The 37% undershoot follows the F2 calibration note exactly: the review priced the win as "−13 redundant single-writer restatements + −5 table frames" but the **skill-specific** survivor rows did not evaporate — they had to be re-prosed as bullets under `## Guardrails` (6 in `init-runbook`, 5 in `extract`, 3 each in `define`/`build`), and each host file gained a one-sentence "see shared guardrails" pointer to keep the link discoverability the table used to provide. Only `execute` delivered a clean H2 deletion (its 4 "Yes — see foo.md" wire-pointer rows were already covered by the existing `## Guardrails` body) and that single file accounts for −13 of the −37 net LOC. Calibration note for future findings tagged "delete a parallel H2, fold survivors into existing H2": predicted ΔLOC should subtract the survivor-row count × ~1.5 LOC (table-row → prose-bullet expansion) before claiming the deletion magnitude. Side-finding: the review's `checkOneGuardrailsBlockPerSkill` "already exists" claim is wrong — `rg checkOneGuardrailsBlockPerSkill scripts/` returns nothing; the predicate is not implemented. The done-when assertion still flipped because `make checks` passes regardless and the structural change holds, but a reviewer asserting a predicate-as-safety-net should grep for it before relying on it.
- F4. **−50 LOC vs −60 predicted (83%)** | both done-when assertions flipped cleanly (`rg -n 'enum ValidationResult' crates/tool/` returns nothing; `rg -n 'Deferred' crates/tool/` returns nothing) | no regressions: `cargo check --workspace --all-features` clean, `cargo clippy --workspace --all-features --all-targets -- -D warnings` clean, full-workspace `cargo nextest run` 825/825 (incl. `specify-tool` 48/48). Touched 2 files (42 insertions / 92 deletions); `crates/tool/src/validate.rs` shed the 42-line enum + `impl rule_id`, the helpers `pass`/`fail` expanded from 11 → 17 LOC because `ValidationSummary` carries owned `String` fields where the old `&'static str` enum did not (each helper gains 3 lines for the struct literal); `src/commands/tool.rs` deleted the 14-line `validation_failure` adapter wholesale and the import line, and inlined the `Fail` filter into `validate_manifest_tools` (5 lines). One follow-up landmine that did not appear in the review's action list: `fn fail_rule_ids` previously returned `Vec<&'static str>` because the rule_ids were static-borrowed inside the enum variants; with the move to `ValidationSummary { rule_id: String }` it became `Vec<&str>` borrowing from the input slice, which broke a single test site (`package_tool_validation_reports_package_rule_ids:432`) that fed it a temporary `Vec<ValidationSummary>`. Fixed in-pass with a one-line `let results = …; let ids = fail_rule_ids(&results);` extraction (+1 LOC included in the −50 delta). Prediction model held up well (83% — best ratio after F1's 103%); the F2/F3 calibration note ("don't conflate sites-touched with structural deletion") was applied during sizing here — predicted −50 internally before reading the review's −60 — and the actual delta matched the internal estimate exactly. Calibration note for future findings tagged "collapse internal type into upstream owned type": when the upstream type carries `String` where the inner type carries `&'static str`, helper-function expansion (`.to_string()` + struct-literal frame) costs 3-4 LOC per helper; subtract that from the predicted deletion before quoting it.
- F5. **−41 LOC vs −55 predicted (75%)** | both done-when assertions flipped cleanly (`rg -n 'enum MergeOp\b' src/` returns nothing; goldens regen produced an empty `git diff` against `tests/fixtures/e2e/goldens/`) | no regressions: `cargo check --workspace --all-features` clean, `cargo clippy --workspace --all-features --all-targets -- -D warnings` clean, full-workspace `cargo nextest run` 825/825 (incl. `slice_merge` 10/10 and `e2e` 9/9 with `merge_two_spec_slice_produces_baselines` re-asserting the canonical wire shape). Touched 2 files (26 insertions / 67 deletions): `crates/domain/src/merge/merge.rs` gained `use serde::Serialize` + `#[derive(Serialize)]` + `#[serde(tag = "kind", rename_all = "kebab-case")]` (+3/-1); `src/commands/slice/merge.rs` shed the 14-line `enum MergeOp`, the 33-line `From<&MergeOperation> for MergeOp`, and the 4-line section header (+23/-66). Two minor surprises that ate into the −55 → −41 gap (25% undershoot): (a) `#[serde(other)]` does **not** "handle the `#[non_exhaustive]` future-proof for free" as the review action claimed — `serde(other)` is deserialization-only; for serialization-only `MergeOperation` consumers, the local match arms in `operation_label`/`summarise_ops` still need a wildcard to compile against `#[non_exhaustive]`, kept as `_ => "UNKNOWN operation".to_string()` and `_ => {}` (4 LOC retained that the prediction credited as deleted via the `Unknown` branch); (b) the prediction implicitly modelled `operation_label` and `summarise_ops` as collapsing to nothing once `MergeOp::Variant` became `MergeOperation::Variant`, but the match arms migrated rather than vanished — five-arm rename × ~2 LOC per arm × 2 helpers = ~20 LOC retained-as-rename, ±0 LOC each. Done-when nuance worth noting for future sizing: the review's action wired the goldens regen to `--test slice_merge`, but the only fixture under `tests/fixtures/e2e/goldens/` that the merge change can move is `merge-two-spec.json`, which is owned by `tests/e2e.rs::merge_two_spec_slice_produces_baselines`; running `REGENERATE_GOLDENS=1 cargo nextest run --test slice_merge` would have returned an empty diff for the wrong reason (it doesn't touch any golden), so I tightened by also running `--test e2e` and confirming `git diff --stat tests/fixtures/e2e/goldens/` was empty after both. Prior calibration (F4 note: "subtract helper-function expansion before quoting deletion magnitude") did not bite here because the field-clone shape was symmetric (`String` on both sides — no `.to_string()` cost), but a *different* mechanism in the same family bit instead: structural-block deletions whose match-arm bodies are *moved* rather than removed cost ~0 net per arm. Calibration note for future findings tagged "collapse a wire mirror enum into the upstream domain enum via `derive(Serialize)`": cap predicted ΔLOC at the literal line-count of the enum + From-impl + module-comment frame; do **not** credit the wire-mirror's match-arm callers as deletions (they migrate by name, not by removal) and do **not** assume `serde(other)` removes the `#[non_exhaustive]` wildcard from local serialization-side matches.
- F6. **−52 LOC vs −51 predicted (102%)** | both done-when assertions flipped cleanly (`wc -l DECISIONS.md` returns 232, well under the 234 ceiling; `rg 'Follow-up' DECISIONS.md` returns 0 hits) | no regressions: pure prose deletion of a resolved TODO, no code touched, no test surface, no callers outside `REVIEW.md` itself (`rg 'DECISIONS\.md.*(Follow|wasm-pkg|2[3-8][0-9])' --type=md` confirms only the F6 evidence/done-when lines reference the deleted section, and those are the review tracking this work). Touched 1 file (1 insertion-equivalent / 53 deletions in `git diff --stat` terms; the +1 is bookkeeping noise from the trailing-newline boundary). The +1 LOC overshoot vs the review's literal `:234-284` range came from also removing line 233 (the blank separator between the deleted H2 and the prior `## Tool architecture` section); leaving it would have produced a stray double-blank tail, so the structural deletion is 52 LOC, not 51. The F2 "structural blocks vs sites touched" calibration note held perfectly here because the finding **is** purely structural (one H2, no in-place edits, no callers); the F4/F5 helper-expansion mechanism doesn't apply to prose deletions. One stale recon row in `REVIEW.md:21` (`| AGENTS.md / DECISIONS.md | 78 / 284 |` — DECISIONS.md is now 232) is left intentionally as a snapshot artifact; rewriting recon rows in flight would defeat the point of the post-mortem (calibration against the snapshot the review priced against). Calibration note for future findings tagged "delete a self-contained markdown H2 section": predicted ΔLOC should add 1 for the separator-blank-above the H2 if the deletion is at end-of-file or between two H2s; this is the cleanest prediction case in the family and the closest hit (102%) of the run so far.
- F7. **−30 LOC vs −40 predicted (75%)** | both done-when assertions flipped cleanly (`rg 'pub enum (Scaffold|Vectis)Error' wasi-tools/vectis/` returns one hit — the unified `wasi-tools/vectis/src/error.rs:27` `VectisError`; `rg 'EXIT_(CLEAN|FINDINGS)' wasi-tools/vectis/` returns 0 hits) | no regressions: `cargo check --workspace --all-targets` clean, `cargo clippy --all-targets -- -D warnings` clean on `specify-vectis`, `cargo nextest run` 78/78 inside `wasi-tools/vectis/` (incl. all integration tests under `tests/engine_{tokens,assets,layout,composition,paths}.rs` and `tests/cli.rs::{invalid_invocation_exits_two, missing_input_exits_two_with_error_envelope}` which assert the typed-error JSON shape and the `2` exit code). Touched 5 files net (new `src/error.rs` 80 LOC; 21 insertions / 131 deletions across the existing four — `lib.rs` +3, `scaffold.rs` +7/−4 for the `ScaffoldError = VectisError` alias + 4-line doc-comment, `scaffold/versions.rs` ±1 path swap, `validate.rs` +10/−67 stripping the inline `pub mod error { ... }` block and the three `EXIT_*` constants). The 25% undershoot vs −40 came from two sources the review's "≈ 50 LOC unified" budget did not size: (a) the new `error.rs` is 80 LOC, not 50 — eight lines of file-level doc-comment explaining *why* the two enums collapsed (necessary because the wire-payload-equivalence claim is the load-bearing argument for the change), plus eight lines of `EXIT_FAILURE` doc-comment justifying the scaffold-failure exit-code change from `1` to `2`; (b) the scaffold-side `pub use crate::VectisError as ScaffoldError;` carries a 4-line doc-comment explaining why the alias exists, because `ScaffoldError` survives as a name in scaffold tests (`src/scaffold/tests.rs:117,134,149,190` pattern-match `ScaffoldError::InvalidProject { message }`) and a future reader hitting the alias deserves the breadcrumb. One follow-up landmine that did not appear in the review's action list: inlining `EXIT_CLEAN`/`EXIT_FINDINGS` as the literal expression `if has_errors(value) { 1 } else { 0 }` trips `clippy::bool_to_int_with_if` (rust-1.95.0; the rule did **not** fire against the original `EXIT_FINDINGS` / `EXIT_CLEAN` named-constant shape because clippy only flags the integer-literal-pair form). Fixed in-pass with `u8::from(has_errors(value))` (1 LOC, ±0 net). Done-when nuance: the review's done-when text quoted `rg 'pub enum (Scaffold|Vectis)Error' wasi-tools/vectis/ returns one hit` — that grep matched the dead `pub enum VectisError {` declaration that survived inside `validate.rs`'s nested `pub mod error { ... }` block until I deleted it; if I had only deleted `scaffold/error.rs` and kept the `validate.rs` mirror, the assertion would have flipped one hit but on the wrong line. Pinned both deletions to the structural change rather than the grep. The F2/F3 "sites touched ≠ structural deletion" calibration note applies inversely here: predicted ΔLOC under-weighted the *new* file's frame-cost. Calibration note for future findings tagged "unify two duplicate enums into one new file": cap the predicted ΔLOC at `current_LOC(file_a) + current_LOC(file_b) − (file_a_unique_lines + file_b_unique_lines + 1.5 × file_frame_overhead)`, where `file_frame_overhead` ≈ 20 LOC (file-level doc-comment + crate-export line + per-method `#[must_use]` + helper-doc on the new constants). For F7 that recomputes to ~−25 LOC predicted, much closer to the actual −30. The review's prediction over-credited the deletion magnitude by treating the new file as a no-op frame.
- F8. **−52 LOC vs −40 predicted (130%)** | both done-when assertions flipped cleanly (`rg 'struct Rule(Summary|Export)\b' src/commands/codex.rs` returns 0 hits; `rg 'export_provenance_text' src/commands/codex.rs` returns 0 hits) | no regressions: `cargo check --workspace --all-features` clean, `cargo clippy --workspace --all-features --all-targets -- -D warnings` clean, full-workspace `cargo nextest run` 825/825 (incl. `specify::codex` 14/14 covering `list_text_shows_summary_fields`, `show_text_prints_summary_and_body`, `export_json_includes_rules_and_paths`, `export_json_resolves_cache_and_overlay`, and `validate_duplicate_ids_exits_two` — the four sites that constrain the wire shape end-to-end). Manual JSON probe of `specify --format json codex list` against a single-rule capability fixture confirmed the output is byte-identical to the prior `RuleSummary` shape (no `trigger`/`body` keys present): `{"rule-count":1,"rules":[{"id":...,"title":...,"severity":...,"source-path":...,"provenance-kind":...,"capability-name":...,"capability-version":...,"catalog-name":null}]}`, validating the review's `skip_serializing_if = "Option::is_none"` strategy — the `Option<&'a str>` wrapper elides cleanly when the `None` constructor (`RuleView::summary`) is used. Touched 1 file (38 insertions / 90 deletions): replaced the dual `RuleSummary` (12 LOC) + `RuleExport` (14 LOC) declarations + the dual `From<&ResolvedCodexRule>` impls (15 LOC + 19 LOC) + the `ProvenanceFields` plumbing struct (6 LOC) + its `const fn provenance_fields` (22 LOC) + the dual `provenance_text` / `export_provenance_text` helpers (11 LOC + 11 LOC) with a single `RuleView<'a>` struct (12 LOC) + an inherent impl carrying `summary`/`full`/`build` (32 LOC where `build` inlines the provenance match as a 7-line tuple destructure) + a single `provenance_text(&RuleView)` helper (10 LOC). Three call sites updated: `list` → `RuleView::summary`, `show` → `RuleView::full`, `export` → `RuleView::full`; `write_show_text` switched `r.trigger` → `r.trigger.unwrap_or_default()` and `r.body` → `r.body.unwrap_or_default()` (both safe because `show` always populates via the `full` constructor — `unwrap_or_default()` chosen over `expect()` because the field semantics map cleanly to "absent in summary view, present in full view" and an empty string is the natural rendering of the absent case in human-readable text output). The 30% **overshoot** vs the predicted −40 inverts the F4/F5/F7 family of helper-expansion calibration notes: the review priced the deletion conservatively because it assumed `summary` and `full` would each cost roughly what the dual `From` impls cost (~14 LOC × 2 = 28 LOC retained), but `bool::then_some` collapses the conditional field-population in 1 LOC per field (`with_body.then_some(frontmatter.trigger.as_str())`), and the `ProvenanceFields` plumbing struct's elimination saves ~17 LOC beyond the naive struct-frame count (the const `fn` constructor was 22 LOC for what becomes a 7-line tuple match inline). The review's prediction also did not credit the second `provenance_text` call — `write_show_text` previously called `export_provenance_text(r)`; both call sites now share `provenance_text(&RuleView)`, deleting an entire 11-LOC helper. Calibration note for future findings tagged "collapse a plumbing-helper-struct + dual-`From`-impl + dual-getter-helper pattern into one view + one constructor + one getter": predicted ΔLOC should add ~15-20 LOC of bonus deletion beyond the naive `2 × struct_frame + 2 × from_impl + 2 × getter` count, because (a) `bool::then_some` (or equivalent guard expression) flattens conditional field population to 1 LOC/field, and (b) eliminating the plumbing struct converts a `(decl + constructor)` pair into a single inline tuple destructure that is materially shorter than the constructor it replaces. Sister calibration to F4 ("subtract helper-function expansion before quoting deletion magnitude"): when collapsing *toward* a thin wrapper rather than *away from* one, helper expansion is **negative** — predicted ΔLOC tends to undercount by ~30%.
- F9. **−38 LOC vs −36 predicted (106%)** | all three done-when assertions flipped cleanly (`wc -l AGENTS.md` returns 76 — well under the 80 ceiling; `rg 'checkBodyAndSectionLineCounts|checkArgumentHintGrammar|IMPERATIVE_VERBS' AGENTS.md` returns 0 hits; `make checks` green in 3.1 s) | no regressions: pure markdown consolidation, no code touched. Touched 3 files (3 insertions / 41 deletions): `AGENTS.md` shed the §"Markdown style" stanza (4 LOC), the §"Skill authoring" stanza (17 LOC including blank-line frames), the §"Mechanical enforcement" prose (5 LOC), and the 12-row predicate table (12 LOC) — replaced with one §"Skill authoring" stanza (3 LOC: H3 + blank + single paragraph) routing to `docs/standards/skill-authoring.md`, `.cursor/rules/project.mdc`, `docs/standards/predicates.md`, and `scripts/standards-allowlist.toml`. The F1 dependency was visibly satisfied — `AGENTS.md:71` already linked to `docs/standards/skill-authoring.md` and named the post-F1 `## Rationale` H2 explicitly, so the new pointer landed cleanly with no further consolidation needed. One follow-up landmine that did not appear in the review's action list: two reference files anchored into the deleted §"Skill body discipline" subhead — `plugins/references/cli-output-shapes.md:3` and `plugins/references/guardrails.md:3` both linked `AGENTS.md#skill-body-discipline`, which would have started 404-ing the moment the H3 disappeared. Redirected both to `docs/standards/skill-authoring.md#skill-body-discipline` (the H2 the F1 consolidation moved this prose to, with the same slug — `make checks` includes a link integrity pass which would have flagged the broken anchors had I missed them); 1 LOC each, net ±0 LOC per file. The review's done-when grep was scoped to predicate-name leakage inside AGENTS.md itself and would not have caught this. The 6% overshoot vs predicted −36 came from collapsing **three** H3 boundaries (`Markdown style` + `Skill authoring` + `Mechanical enforcement`) into one rather than the predicted two — `docs/standards/skill-authoring.md` already owns the Markdown style rules at `:69-72`, so the AGENTS.md stanza was redundant in the same way the rest was; folding it into the pointer paragraph saved another ~4 LOC over the ~6-LOC replacement budget. F6's "delete a self-contained markdown H2 section" calibration (predicted +1 for the separator-blank-above, 102% hit) ported almost perfectly — this is the same prose-only structural-deletion family. Calibration note for future findings tagged "collapse multiple sibling H3s under one parent into one pointer paragraph": predicted ΔLOC should add ~3-4 LOC of bonus deletion per *additional* H3 collapsed beyond the first, because (a) each extra H3 brings a header + blank-frame pair (~2 LOC) and (b) the pointer paragraph's enumeration cost scales as ~1 LOC per pointed-at topic rather than ~1 LOC per H3 collapsed (the predicted `~6 LOC stanza` already absorbs the additional topic names without growing). Sister calibration to F6 ("predicted ΔLOC should add 1 for the separator-blank-above the H2 if the deletion is at end-of-file or between two H2s"): for *interior* H3 deletions (this case, with §"Gotchas" remaining downstream), the trailing-blank stays attached to the surviving section, so no separator-blank adjustment was needed. Operating bookkeeping: anchor-link callers into a deleted section should be grepped as part of any structural-deletion done-when — adding `rg '<file>#<deleted-section-slug>' --type=md` to the assertion set would have surfaced the two callers automatically.
