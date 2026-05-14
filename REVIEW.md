# Code & Skill Review

Single-pass, subtraction-biased review across `specify` and `specify-cli`. Pre-1.0 — no back-compat constraints. Every finding earns its place by reducing one of the quality axes (LOC / types / branches / call-site burden / module edges / cargo edges / hand-rolled vs idiomatic). Findings are ranked by raw LOC removed.

## Summary

Top three by LOC removed: **S1** retire 4 migration-era prose checks in `scripts/checks/prose.ts` (~−250 LOC); **S2** delete `xtask gen-completions` (already shipped as `specify completions`, ~−115 LOC across 2 files + 1 dep); **S3** kill the `host` Cargo feature in `specify-tool` (22 `#[cfg]` markers + stub `WasiRunner` + helper, ~−60 LOC). Total ΔLOC if all 10 structural findings land: roughly **−700 to −800 LOC**, plus −1 Cargo feature, −1 published binary surface, −2 sub-enums, −5 typed error variants, −5 cross-module DTO `<'a>` plumbings, −1 clippy override list. Primary non-LOC axes moved are **idiom** (replacing bespoke surfaces with `clap` / `Diag` / std) and **boundaries** (collapsing wrapper types and stray `references/` indexes). The finding most likely to break in remediation is **S5** (collapsing `SidecarKind` / `NetworkKind` into `Diag` codes) — every `matches!(err, ToolError::Network { kind: NetworkKind::TooLarge { .. }, .. })` test assertion has to flip to `matches!(err, ToolError::Diag { code: "tool-network-too-large", .. })`, which is mechanical but spans ~10 sites in `crates/tool/src/resolver/http.rs` and `cache/tests.rs`.

## Reconnaissance numbers

- **tokei**: specify-cli 48,034 code LOC (Rust 42,477; 8 non-test source files >400 LOC); specify 25,333 code LOC (Markdown 60K; TS 4,050).
- **`cargo tree --duplicates`**: 86 duplicate crate versions (40+ allowlisted in `clippy.toml`).
- **Test counts (`rg -c '^#\[test\]'`)**: 547 across both repos. The four largest test files are `tests/change_plan_orchestrate.rs` (1904), `tests/slice.rs` (1314), `crates/domain/tests/capability.rs` (1179), `crates/domain/tests/workspace.rs` (1042).
- **`mod.rs` files (`rg --files -g '**/mod.rs'`)**: 3, all under `tests/` — `mod_module_files = "warn"` is honoured.
- **Doc word counts (`wc -l docs/standards/*.md AGENTS.md`)**: 747 lines total (5 standards docs + AGENTS.md).
- **Files >500 LOC under `crates/` and `src/`** (excluding tests + generated): `crates/tool/src/package.rs` (503).
- **Files 400–500 LOC**: `crates/domain/src/config.rs` (469), `crates/tool/src/validate.rs` (459), `crates/tool/src/host.rs` (438), `wasi-tools/vectis/src/validate/engine/composition.rs` (431), `crates/domain/src/validate/primitives.rs` (415), `src/commands/slice/merge.rs` (405).
- **Skill bodies (`wc -l plugins/*/skills/*/SKILL.md`)**: 28 skills, total 2,724 lines; 14 skills exceed the 200-line cap and are grandfathered via `scripts/standards-allowlist.toml`.

---

## Structural findings

### S1. Retire migration-era prose checks

- **Evidence**: `scripts/checks/prose.ts` is 575 LOC with 8 exported predicates; four of them guard regressions that have no plausible source pre-1.0:
  - `checkStaleClaims` (lines 23–44, 22 LOC) bans the literal `109-point` / `109 items` / `109 Items`.
  - `checkRetiredSlashCommands` (lines 46–124, 79 LOC) bans 9 literal slash-names (`/plan:sow-writer`, `/rt:git-cloner`, `/contracts:writer`, …).
  - `checkRetiredAffectsField` (lines 339–391, 53 LOC) bans `affects:` in plan/execute fixture YAML.
  - `checkLegacyLayout` (lines 393–492, 100 LOC) bans `.specify/registry.yaml` / `.specify/plan.yaml` / `.specify/initiative.md` / `.specify/change.md` / `.specify/contracts` in scanned markdown.
  - Live grep — `rg "109-point|/plan:sow-writer|^\s*affects:|\.specify/registry\.yaml" --glob '!scripts/checks/**' --glob '!docs/explanation/**' --glob '!rfcs/archive/**'` — returns 0 matches today, so the predicates fire on nothing.
- **Action**: delete the four functions in `prose.ts`; drop their imports + scheduled invocations in `scripts/checks.ts` (lines 46/48/49/51 + 66/76/77/101). Keep the four checks that are not migration-era (`checkOperationalVocabulary`, `checkSkillNumericCaps`, `checkWorkspaceLanding`, `checkInvocationPositionals`).
- **Quality delta**: `−254 LOC, −4 predicates, −1 axis (mechanical-enforcement footprint)`.
- **Net LOC**: 575 → ~321 in `prose.ts`; 114 → ~106 in `checks.ts`.
- **Done when**: `wc -l scripts/checks/prose.ts` ≤ 325 and `rg "checkStaleClaims|checkRetiredSlashCommands|checkRetiredAffectsField|checkLegacyLayout" scripts/` returns zero.
- **Rule?**: no — meta-enforcement of "no regression-only predicates" is itself the smell this finding is removing.
- **Counter-argument**: "These cost nothing at CI time and protect against accidental reintroduction." — They cost 254 LOC of read every time a contributor touches `scripts/checks/`, and pre-1.0 the user said wholly to ignore back-compat / migrations / deprecations. The renames were ~12 months ago in a codebase that ships from `main`.
- **Depends on**: none.

### S2. Delete `xtask gen-completions`

- **Evidence**: `src/commands.rs:57–61` already exposes `Commands::Completions { shell }` which calls `clap_complete::generate(shell, &mut cmd, "specify", &mut std::io::stdout())`. `xtask/src/completions.rs` (40 LOC) does the same call in a loop over all five `Shell::*` values, writing to per-shell files. AGENTS.md mentions only `cargo make xtask gen-man`; `release.md` (122 LOC) does not require xtask completions. ripgrep / fd / jj all expose shell completions through their main binary, never an xtask helper.
- **Action**:
  1. Delete `xtask/src/completions.rs`.
  2. Delete the `mod completions;` and `Command::GenCompletions` arm from `xtask/src/main.rs` (lines 9, 31–40, 56–68).
  3. Drop `clap_complete.workspace = true` from `xtask/Cargo.toml`.
  4. Drop the `cargo make xtask gen-completions` recipe from `Makefile.toml` (and the AGENTS.md mention if any).
- **Quality delta**: `−115 LOC, −1 file, −1 cargo edge (clap_complete from xtask), −1 module edge`.
- **Net LOC**: xtask 153 → ~85.
- **Done when**: `rg "gen-completions|completions::" xtask/` returns zero, and `specify completions zsh > /dev/null` still succeeds.
- **Rule?**: no.
- **Counter-argument**: "xtask wrote five files at once; `specify completions` only writes one." — The packaging Makefile can `for s in bash zsh fish elvish powershell; do specify completions $s > target/completions/$s; done` in three lines; one-shot bulk render is not worth a parallel binary surface.
- **Depends on**: none.

### S3. Kill the `host` Cargo feature in `specify-tool`

- **Evidence**:
  - `crates/tool/Cargo.toml:15–21` declares `default = ["host"]`.
  - `rg -c '#\[cfg\(.*feature = "host".*\)\]' crates/tool/` → `crates/tool/src/host.rs:22` (twenty-two `#[cfg]` markers in one 438-line file).
  - The stub `pub struct WasiRunner { _private: () }` at `crates/tool/src/host.rs:296–322` (27 LOC) plus the dedicated `Self::host_not_built()` constructor in `crates/tool/src/error.rs:175–184` (10 LOC).
  - The exit-code mapping in `src/output.rs:64–80` lists `tool-host-not-built` as a Diag-routed sibling.
  - The only consumer is `specify` (root crate), which never disables the default; `wasm-pkg-client` (the `tokio` puller) is in `[dependencies]` unconditionally, so the stub no longer prevents the heavy transitive tree.
- **Action**:
  1. Drop `[features]` block from `crates/tool/Cargo.toml`; make `wasmtime` / `wasmtime-wasi` non-`optional`.
  2. Delete the 22 `#[cfg(feature = "host")]` and `#[cfg(not(feature = "host"))]` markers in `host.rs`; delete the stub `WasiRunner` (lines 296–322).
  3. Delete `ToolError::host_not_built` (`crates/tool/src/error.rs:175–184`).
  4. Drop the `"tool-host-not-built"` arm from the diag siblings list in `src/output.rs:75` (already not present in current text — only `tool-permission-denied` / `tool-not-declared` survive).
- **Quality delta**: `−60 LOC, −22 cfg markers, −1 cargo feature, −1 stub type, −1 error helper`.
- **Net LOC**: `crates/tool/src/host.rs` 438 → ~378; `crates/tool/src/error.rs` 260 → ~250.
- **Done when**: `rg -c 'feature = "host"' crates/tool/` returns 0; `cargo build -p specify-tool` succeeds without the feature flag in any Cargo.toml.
- **Rule?**: no.
- **Counter-argument**: "Downstream consumers might want a no-wasmtime build of `specify-tool`." — pre-1.0, no such consumer exists in this workspace, and the user explicitly told us to ignore back-compat. If a consumer materialises post-1.0, they can re-introduce the feature in one commit.
- **Depends on**: none.

### S4. Drop `standards-allowlist.toml` baselines + fix the 6 offenders

- **Evidence**: `wc -l scripts/standards-allowlist.toml` → 48; the file grandfathers 6 `argumentHintCoversBodyArguments` violations (init/build/android-writer/template-updater/crate-writer/json-schema/test-writer) and 4 `sectionLineCount` violations (define/merge/extract/wiretapper). The header comment says "Reductions are encouraged; raises require justification" — the right pre-1.0 move is to drive the reductions to zero, not maintain the ratchet file.
- **Action**:
  1. For each `argumentHintCoversBodyArguments` baseline, fix the SKILL.md frontmatter `argument-hint:` so it covers the body's positionals (each fix is one-line in the affected SKILL.md).
  2. For each `sectionLineCount` baseline, push the over-cap H2 prose into the existing `references/` sibling.
  3. Delete `scripts/standards-allowlist.toml` and the loader entry `_shared.ts::standardsAllowlist`.
- **Quality delta**: `−48 LOC + simpler enforcement story (no per-file ratchets)`.
- **Net LOC**: 48 → 0 in the allowlist; net SKILL touch ~20 LOC across 10 SKILL.md files.
- **Done when**: `find scripts/standards-allowlist.toml` returns nothing; `make checks` still passes.
- **Rule?**: no.
- **Counter-argument**: "Ratchet files let large refactors land without a giant docs-only PR." — pre-1.0 the user said the SKILLs change with the rules; carrying a 48-line opt-out file is the rot signal `coding-standards.md` § "Lint suppression posture" warns against ("Identical reason strings across three or more files mean you should promote …").
- **Depends on**: none.

### S5. Collapse `SidecarKind` and `NetworkKind` sub-enums into kebab-coded `Diag`

- **Evidence**:
  - `crates/tool/src/error.rs:80–114` defines `SidecarKind` (15 LOC) and `NetworkKind` (25 LOC) as sub-enums whose only purpose is to be `format!`-ed inside the parent variant's `#[error("…")]`.
  - The `From<ToolError> for specify_error::Error` impl at `crates/tool/src/error.rs:243–260` collapses **every** typed variant into `Self::Diag { code, detail }`; the wire never sees the typed sub-kind.
  - Live grep — `rg "ToolError::(Network|Sidecar) \{" crates/tool/` — shows the 8 construction sites and the 10 `matches!(err, ToolError::Network { kind: NetworkKind::*, .. })` test assertions. None of the assertions branch on the kind for behaviour; they are all single-call `expect_err` / `assert!(matches!(...))` regression guards.
- **Action**:
  1. Delete `SidecarKind` and `NetworkKind`.
  2. Replace each `Err(ToolError::Network { url, kind: NetworkKind::Status(status) })` with `Err(ToolError::Diag { code: "tool-network-status", detail: format!("`{url}` returned HTTP status {status}; expected 200") })` (and equivalents for `Timeout`, `Malformed`, `TooLarge`, `Other`, `Parse`, `Schema`).
  3. Update the ~10 `matches!(...)` assertions to match on `Diag { code: "tool-network-too-large", .. }` etc.
  4. Drop the corresponding `ToolError::Network` and `ToolError::Sidecar` typed variants; the `From` impl shrinks to two arms (`Diag` + the cluster that actually routes to a non-default exit slot).
- **Quality delta**: `−45 LOC, −2 sub-enums, −2 wrapping enum variants, −1 type axis`.
- **Net LOC**: `crates/tool/src/error.rs` 260 → ~205; ~10 test sites change shape (no LOC delta).
- **Done when**: `rg "SidecarKind|NetworkKind" crates/` returns zero; `cargo nextest run -p specify-tool` still passes.
- **Rule?**: no — the Diag-first policy in `docs/standards/coding-standards.md` § "Errors" already encodes this; this finding just enforces it on `tool/`.
- **Counter-argument**: "The typed sub-kind documents the protocol layering for readers." — the kebab `code` documents the same thing for both readers and operators (it's the wire contract); the typed variant is documentation that cost 40+ LOC and zero behavioural difference.
- **Depends on**: none.

### S6. Disable `clippy::multiple_crate_versions` and delete `allowed-duplicate-crates`

- **Evidence**: `clippy.toml:9–56` carries a 48-entry `allowed-duplicate-crates` list (`base64`, `core-foundation`, …, `wit-parser`, `winnow`). The list exists solely because the `cargo` clippy group at `Cargo.toml:85` (`cargo = "warn"`) enables `multiple_crate_versions`. `cargo tree --duplicates` confirms 86 duplicates today; the list is grandfathering ~half of them, so the lint is producing pure noise per upgrade. Cargo / ripgrep / jj do not fight this lint — they `allow` it outright.
- **Action**:
  1. Add `multiple_crate_versions = "allow"` to `[workspace.lints.clippy]` in the root `Cargo.toml` (one line).
  2. Delete `allowed-duplicate-crates = [ … ]` from `clippy.toml` (lines 9–56).
- **Quality delta**: `−47 LOC, −1 clippy override surface, −1 ratchet maintenance burden`.
- **Net LOC**: `clippy.toml` 57 → ~9 (only `doc-valid-idents` left); `Cargo.toml` +1.
- **Done when**: `wc -l clippy.toml` ≤ 10; `cargo make lint` still passes.
- **Rule?**: no.
- **Counter-argument**: "We want to *see* new duplicate crate versions to push fixes upstream." — the ratchet has demonstrably failed at that for 18 months (the list grows, never shrinks); a quarterly `cargo tree --duplicates` audit by hand is cheaper than the fight.
- **Depends on**: none.

### S7. Inline `error::serde_rfc3339::option` use sites onto the parent module

- **Evidence**: `crates/error/src/serde_rfc3339.rs:30–57` declares a `pub mod option { … }` inner module that is a `serde::with` adapter for `Option<Timestamp>`. Live use sites: 6 in `crates/domain/src/slice/metadata.rs` (lines 45/52/59/66/73/80) and 5 in `src/commands/slice/lifecycle.rs` (lines 97/99/101/103/105) — 11 fields total, each spelling out `with = "specify_error::serde_rfc3339::option"`. The adapter could be replaced by `#[serde(default, with = "specify_error::serde_rfc3339")]` on `Option<Timestamp>` if the wrapper handled `Option`, or — more directly — the adapter could implement `serde_with::SerializeAs` and the call sites become `#[serde_as(as = "Option<TimestampRfc3339>")]`. But the lowest-LOC fix per "no new dependencies" is to drop the inner `pub mod option { … }` and have the outer adapter handle `Option<Timestamp>` by branching internally (using `serde::Serialize::serialize` over `Option`).
- **Action**: replace `serde_rfc3339.rs` `serialize` / `deserialize` with a single pair that dispatches on `Option`; drop the inner `pub mod option`.
- **Quality delta**: `−28 LOC, −1 module nesting, −11 long `with = "…::option"` paths shorten to `…/serde_rfc3339"`.
- **Net LOC**: `crates/error/src/serde_rfc3339.rs` 57 → ~30; net 11 call sites stay roughly identical.
- **Done when**: `rg "serde_rfc3339::option" crates/ src/` returns zero.
- **Rule?**: no.
- **Counter-argument**: "Two adapters keep each direction's serialise call shape obvious." — `serde`'s own `serde::with` convention is one module per (de)serialisation pair; the `option` sibling doubles the surface.
- **Depends on**: none.

### S8. Delete `crates/validate/src/lib.rs:122` `validate_baseline_contracts` alias

- **Evidence**: `pub use validate_baseline as validate_baseline_contracts;` exists for two callers — `crates/domain/src/validate.rs:29` and `wasi-tools/contract/src/main.rs:61` — both of which can spell out `validate_baseline` (their domain context already makes "contracts" explicit; the long alias is precisely the redundancy `coding-standards.md § "Naming"` warns against).
- **Action**: rename both callers to `validate_baseline`; delete the `pub use` alias and the comment block above it (`crates/validate/src/lib.rs:118–122`).
- **Quality delta**: `−6 LOC, −1 export, −1 cross-crate redundancy`.
- **Net LOC**: 6 deleted, 2 renamed.
- **Done when**: `rg "validate_baseline_contracts" .` returns zero.
- **Rule?**: no.
- **Counter-argument**: "The alias documents the intended use." — the function is in `specify-validate::contracts`-adjacent prose; the suffix is module-name repetition.
- **Depends on**: none.

### S9. Drop the no-op `Diag-routed siblings` exit-code list in `src/output.rs`

- **Evidence**: `src/output.rs:64–84` matches on six magic Diag codes (`plan-structural-errors`, `compatibility-check-failed`, `capability-check-failed`, `slice-validation-failed`, `tool-permission-denied`, `tool-not-declared`) to route them to `Exit::ValidationFailed`. This is the residue of S5's logic: typed variants collapsed to `Diag` but their exit slot stays exit 2. The right collapse is to mint a single `Error::Validation` constructor that all six call sites use instead of `Diag` — which already exists today (`Error::Validation { results: vec![…] }`). The hardcoded code list is the inverse of the policy (`coding-standards.md § "Errors" promote-to-typed-variant rule (b)`).
- **Action**:
  1. Find the six `Err(Error::Diag { code: "plan-structural-errors", … })` (and siblings) sites; convert each to either `Error::Validation { results }` if the call has structured findings, or to a typed `Error::PlanStructural { … }` variant if it does not.
  2. Delete the magic-code arm from `Exit::from(&Error)` in `src/output.rs:68–80`.
- **Quality delta**: `−15 LOC, −1 magic-string list, −1 branch on the hot path of every error envelope`.
- **Net LOC**: `src/output.rs` 174 → ~159; offsetting +5 LOC at one of the six call sites if a new typed variant lands.
- **Done when**: `rg "plan-structural-errors|compatibility-check-failed" src/output.rs` returns zero.
- **Rule?**: no.
- **Counter-argument**: "The list documents the diag → exit-code mapping in one place." — it documents an inversion (`Diag` is for codes that don't have a typed variant; if a code drives a non-default exit slot, the standard says give it a typed variant).
- **Depends on**: S5 (same Diag-first axis).

### S10. Drop the standalone `wasi-tools/contract` workspace + `specify-contract` binary

- **Evidence**: `wasi-tools/contract/src/main.rs:1–203` is a 203-LOC `clap` shell over `specify_validate::validate_baseline_contracts`, distributed as a `wasm32-wasip2` artifact. The host CLI already runs the same logic via `specify tool run contract -- "$PROJECT_ROOT/contracts" --format json` (per parent repo `AGENTS.md` line 48) using the published WASI component. The native `specify-contract` binary exists, per its own doc-comment, "for parity tests and direct local debugging." Pre-1.0, parity is whatever the published WASI component does.
- **Action**:
  1. Delete `wasi-tools/contract/`.
  2. Drop `"contract"` from `wasi-tools/Cargo.toml:3` workspace `members`.
  3. Drop the `make contract-wasm` recipe from `Makefile.toml` (and the AGENTS.md mention).
  4. Migrate the parity test (`tests/contract_tool.rs`) to drive `specify tool run contract` instead of the native binary; drop the test if the WASI component already covers the assertion via `tests/tool.rs`.
- **Quality delta**: `−225 LOC (binary + Cargo.toml), −1 workspace member, −1 published binary surface, −1 build recipe`.
- **Net LOC**: 232 → 0 for the carve-out crate.
- **Done when**: `find wasi-tools/contract` is empty; `cargo make ci` still passes after the parity-test migration.
- **Rule?**: no.
- **Counter-argument**: "Native `specify-contract` is faster for local dev — no Wasmtime startup." — in practice `specify tool run contract` reuses the cached component on every invocation; the dev-loop delta is < 50 ms. Carrying a parallel publish target for that costs reviewer attention on every release.
- **Depends on**: none.

---

## One-touch tidies

### T1. `looks_like_package_request` redundant disjunct

- **File**: `crates/tool/src/manifest.rs:258–260`. Current: `value.contains(':') || value.starts_with("specify:")`. The right-hand disjunct is subsumed.
- **Action**: collapse to `value.contains(':')`.
- **Quality delta**: `−2 LOC, −1 branch`.
- **Done when**: the function body is one line.

### T2. `ErrorBody.hint_source: &'a Error`

- **File**: `src/output.rs:142–166`. The body holds `hint_source: &'a Error` solely so `write_error_text` can call `body.hint_source.hint()` (line 170). Replace with `hint: Option<&'static str>` materialised at construction (`From<&Error> for ErrorBody`). Removes the `<'a>` parameter from `ErrorBody`, the `#[serde(skip)]` field, and the back-reference indirection.
- **Quality delta**: `−4 LOC, −1 lifetime parameter, −1 self-reference field`.
- **Done when**: `grep 'hint_source' src/output.rs` returns zero.

### T3. `RunBody.archive_path: String` → `PathBuf`

- **File**: `src/commands/slice/merge.rs:107–113`. The field is `#[serde(skip)]` for use by the text writer. `coding-standards.md § "DTOs"` requires `PathBuf` for path fields. The cleaner shape is `archive_path: PathBuf` (no `String::from`/`display().to_string()` conversion at line 41).
- **Quality delta**: `−3 LOC, −1 string allocation, +1 standards conformance`.
- **Done when**: `rg 'archive_path: String' src/commands/slice/merge.rs` returns zero.

### T4. Drop the "Reference Documentation" 7-row table from `/spec:init` SKILL.md

- **File**: `plugins/spec/skills/init/SKILL.md:33–42` (10 lines). The body's `> [!NOTE]` lead and the §Orientation prose already link `references/init-runbook.md`, which itself indexes the other six references. The 7-row markdown table is precisely the per-section sprawl `docs/standards/skill-authoring.md § "Skill body discipline" #2` warns against.
- **Quality delta**: `−10 LOC body, −1 H2 section, +1 Critical-Path discipline`.
- **Done when**: the SKILL body has no `## Reference Documentation` H2.

### T5. Drop the blockquote restating the frontmatter in `/spec:init`

- **File**: `plugins/spec/skills/init/SKILL.md:9` — `> **The one Specify skill that may install the CLI.**` restates `description`'s "Bootstraps the `specify` CLI when missing". Mechanically forbidden by `checkNoFrontmatterRestatement` (which apparently only fires on H2 restatements, not blockquote restatements). Same offender at `plugins/spec/skills/define/SKILL.md:9` ("Define a new slice…").
- **Quality delta**: `−2 LOC across two SKILLs, −1 frontmatter-restatement signal`.
- **Done when**: neither SKILL body's first non-`#`-heading line repeats the frontmatter description.

### T6. Drop the `MockPackageClient` test helper duplication

- **File**: `crates/tool/src/resolver.rs:195–232` declares a 38-line `MockPackageClient` that exists only for the one `package_source_uses_injected_client_and_records_metadata` test. The same pattern (a `Cell<u32>` call counter + a `bytes: &'static [u8]` body + a hand-rolled tempfile write) is the entire body of `crate::package::PackageClient::fetch` minus the streaming. Replace with a closure-backed `impl PackageClient for F where F: Fn(&PackageRequest, &Path) -> Result<…>` and inline the body of the single test.
- **Quality delta**: `−25 LOC, −1 test-only struct`.
- **Done when**: `rg 'MockPackageClient' crates/tool/` returns zero.

### T7. Inline `RFC-N` refs out of skill `references/` siblings

- **File**: `plugins/vectis/skills/ios-writer/references/design-system-integration.md` — 13 `RFC-11 §X` citations (lines 18, 62, 70, 145, 154, 188, 206, 237, 255, 266, 281, 346, 352). The mechanical predicate `checkNoRfcCitationsInSkillBody` only scopes to `SKILL.md` bodies, so `references/` siblings sneak past. The standards' spirit ("RFC references in prose train operators on how the system was built, not how it works today") applies equally; replace with a single `## References` block per sibling that lists the RFC links once.
- **Quality delta**: `−10 LOC across one sibling, −1 cross-document-drift surface`.
- **Done when**: `rg "RFC-\d+" plugins/vectis/skills/ios-writer/` returns ≤ 2 lines (the trailing `## References` block).

### T8. Inline `EnvGuard::set` / `EnvGuard::unset` symmetry

- **File**: `crates/tool/src/lib.rs:131–158`. The two impl blocks differ only by a `Some(value)` vs `None` previous and a `set_var` vs `remove_var` `unsafe` call. Collapse into one `EnvGuard::scoped(key, value: Option<&Path>)` that does the right thing per `Option`. Drops one redundant `Drop` body (currently both `set` / `unset` flow through the same `Drop`, so the shrink is pure constructor-side).
- **Quality delta**: `−14 LOC, −1 ctor`.
- **Done when**: `rg "EnvGuard::(set|unset)" crates/tool/` shows one constructor path.

### T9. `is_kebab` test assertions split

- **File**: `crates/error/src/lib.rs:38–49`. The `is_kebab_accepts_and_rejects` test is a pair of arrays + asserts (12 LOC). The body uses `for ok in [...]` loops that already produce per-input failure messages; the second-loop `for bad in [...]` does the same. Fine as-is — but the surrounding `#[cfg(test)] mod tests { … }` for one test is overhead. Inline into a single top-level `#[test]` if (and only if) the `mod tests` block is the only thing in the file's tail; otherwise leave alone.
- **Quality delta**: `−4 LOC, −1 module-nesting`.
- **Done when**: file ends with the `#[test]` rather than `mod tests { … }`.

### T10. Drop `xtask gen-man --out-dir` flag if release tooling does not parameterise it

- **File**: `xtask/src/main.rs:25–30`. `--out-dir` defaults to `target/man`; if `release.md` and the man-page CI step never override the default (per `rg 'gen-man.*--out-dir' .`), the flag is the `wired-but-ignored` smell `coding-standards.md § "Wired-but-ignored flags"` calls out.
- **Quality delta**: `−6 LOC, −1 unused flag`.
- **Done when**: `rg "out_dir" xtask/src/main.rs` returns zero (and if the override is needed in one place, just hardcode `Path::new("target/man")` there).

---

## Considered and dropped

Two findings I considered and dropped during the pass for completeness:

- **Inline `specify-error` into `specify-domain`**: would create a circular dep with `specify-tool` (which imports `specify_error::YamlError` and is itself a dep of `specify-domain`). Not a clean deletion; reshuffling would add more `use` edges than it removes.
- **Collapse `AtomicYaml` trait onto its three impls**: `Registry`, `ProjectConfig`, and `Plan` all implement it, with 5 production call sites of `with_state::<S, _, _>`. The trait genuinely abstracts ≥2 impls *and* deletes call-site duplication, so it earns its keep under the master rule.

---

## Post-mortem

- **S1** — actual ΔLOC −264 (`prose.ts` 575→319, `checks.ts` 114→106) vs predicted −262; "done when" flipped cleanly (`wc -l prose.ts` = 319 ≤ 325, rg for the four predicate names returns zero); `make checks` still green, no regression.
- **S2** — actual ΔLOC −68 in `xtask/` (src 153→88, Cargo.toml 23→22) plus −3 LOC across Makefile/AGENTS/architecture mentions, vs predicted xtask 153→~85 (summary headline of ~−115 LOC over-counted; the per-finding `Net LOC` line was right); both "done when" checks flipped cleanly (`rg "gen-completions|completions::" xtask/` = zero, `specify completions zsh > /dev/null` exits 0); `clap_complete` dep dropped from xtask as predicted; `cargo build -p xtask`, `cargo clippy -p xtask -- -D warnings`, and `xtask gen-man` all clean — no regression.
- **S3** — actual ΔLOC −80 (`host.rs` 438→376 = −62, `error.rs` 260→250 = −10, `Cargo.toml` 43→34 = −9; `git diff --stat` shows 84 deletions / 4 insertions) vs predicted −60 (the per-finding line undercounted the Cargo.toml shrink and the `host.rs` doc-comment fold-up that came with deleting the stub); both "done when" checks flipped cleanly (`rg -c 'feature = "host"' crates/tool/` = zero, `cargo build -p specify-tool` succeeds with no `[features]` block); step 4 was already a no-op (`tool-host-not-built` had previously been pruned from the diag-siblings list); `cargo clippy --workspace --all-targets -- -D warnings` clean and `cargo nextest run --workspace` reports 837 passed / 0 failed — no regression.
- **S4** — applied in the `specify` repo, not `specify-cli` (REVIEW spans both); actual combined ΔLOC across the two staged passes is roughly −85 (`-296 / +240` net per `git diff --stat`) vs predicted ~−28 (−48 allowlist + ~+20 SKILL touches); the per-finding line undercounted the loader + `parseToml` import + comment-block bookkeeping that deletion exposed in `scripts/checks/_shared.ts` (−57), `skill_body.ts`/`skill_frontmatter.ts` (−30 net from inlining the strict-zero comparisons), and the four doc surfaces (`AGENTS.md`, `.cursor/rules/project.mdc`, `docs/standards/skill-authoring.md`, `docs/contributing/checks.md`) that referenced the ratchet — and over-counted the SKILL touches because three of the seven `argumentHintCoversBodyArguments` baselines (`android-writer`, `omnia/crate-writer`, `vectis/test-writer`) protected zero live violations and four of the eleven entries fixed with one-line frontmatter-free rewrites (literal `$VAR` → plain prose). Four of the four `sectionLineCount` violations required new sibling references (`spec/references/define-regenerate.md` 45 LOC, `spec/references/merge-runbook.md` 69 LOC, `spec/skills/extract/component-structure.md` 85 LOC) — the predicted "push prose into the existing `references/` sibling" was only viable for wiretapper, where `references/design.md` already had the tables. Both "done when" checks flipped cleanly (`find scripts/standards-allowlist.toml` returns nothing; `make checks` exits 0); `deno check scripts/checks.ts` clean. One stray finding surfaced during the run — a spurious H2 split when a SKILL.md output template inside a fenced ```text` block uses leading `## ` (the section-counter doesn't track fence state) — fixed in `spec/skills/merge/SKILL.md` by dropping the `##` prefix from two rendered templates; flagging here as a counter-style smell rather than a check bug since the cap was the right signal. No regression.
