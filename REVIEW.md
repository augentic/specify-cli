# Code & Skill Review — specify + specify-cli

## Summary

**Top three by LOC removed:** S1 collapse `composition::MergeOp` into `MergeOperation` (−55), S2 `FenceError` → `thiserror` (−25), S3 collapse `DiagnosticSeverity` into `Severity` (−22). **Total ΔLOC if all land:** ~−160. **Primary non-LOC axes:** −4 types, −4 match branches, 2× hand-rolled → idiomatic. **Most likely to break in remediation:** S1 — composition test assertions reference the deleted `MergeOp` variants and must be mechanically rewritten to `MergeOperation`.

## Reconnaissance

```
tokei (specify-cli): 273 Rust files, 49,285 lines, 43,169 code
tokei (specify):     541 Markdown files, 59,280 lines (prompt-engineering repo)
cargo tree --duplicates: all dupes are transitive (wasmtime/cranelift/bitflags); no workspace-level action
rg -c '^#[test]': 627 tests across crates/ src/ tests/
rg --files -g '**/mod.rs': 3 files (all in tests/ support dirs — coding-standards rule holds)
wc -l docs/standards/*.md AGENTS.md (cli): 573 total
files > 500 lines: 2 (validate.rs 539, doctor/tests.rs 549) — both < 50% test
```

---

## Structural Findings

### S1. Collapse `composition::MergeOp` into `MergeOperation`

**Evidence:**

`crates/domain/src/merge/composition.rs` defines a 4-variant `MergeOp` enum (lines 17–39, 23 lines) and a `MergeResult` struct (lines 8–14, 7 lines). `crates/domain/src/merge/merge.rs` defines a 5-variant `MergeOperation` enum (lines 34–66). Three of the four `MergeOp` variants (`Added`, `Modified`, `Removed`) mirror `MergeOperation` identically except for field names (`slug` vs `id` + `name`). The fourth (`CreatedBaseline { screen_count }`) mirrors `MergeOperation::CreatedBaseline { requirement_count }`.

`crates/domain/src/merge/slice/read.rs` lines 184–211 spend 28 lines on a `.iter().map(|op| match op { … })` that converts `MergeOp` → `MergeOperation` by cloning `slug` into both `id` and `name`. The two `MergeResult` structs are structurally identical (`output: String, operations: Vec<…>`).

**Action:**

1. Delete `composition::MergeResult` struct and `composition::MergeOp` enum from `composition.rs` (−30 lines).
2. Add `use crate::merge::merge::{MergeOperation, MergeResult};` to `composition.rs` (+1 line).
3. In `composition.rs`, replace each `MergeOp::Added { slug: slug.clone() }` with `MergeOperation::Added { id: slug.clone(), name: slug.clone() }` (3 sites, +3 lines). Replace `MergeOp::CreatedBaseline { screen_count }` with `MergeOperation::CreatedBaseline { requirement_count: screen_count }`.
4. Update 5 test assertions in `composition.rs` from `MergeOp::X { slug }` to `MergeOperation::X { id, name }` (+5 lines).
5. In `read.rs`, replace the 32-line `Ok(comp_result) => { let spec_merge_result = MergeResult { … }; merged.push(…); }` block (lines 181–220) with a direct push of `comp_result` (8 lines, −24 lines).

Before (`read.rs` lines 181–220):
```rust
Ok(comp_result) => {
    let spec_merge_result = MergeResult {
        output: comp_result.output,
        operations: comp_result
            .operations.iter()
            .map(|op| match op { /* 4 arms, 22 lines */ })
            .collect(),
    };
    merged.push(MergePreviewEntry { /* … */ result: spec_merge_result });
}
```

After:
```rust
Ok(comp_result) => {
    merged.push(MergePreviewEntry {
        class_name: class.name.clone(),
        name: "composition".to_string(),
        baseline_path,
        result: comp_result,
    });
}
```

**Quality delta:** −55 LOC, −2 types, −4 branches, −1 module edge.

**Net LOC:** composition.rs 266 → 245; read.rs 378 → 354. Combined 644 → 599 (−45 production, −10 test).

**Done when:** `rg 'MergeOp' crates/domain/src/merge/composition.rs` returns 0 hits.

**Rule?** No.

**Counter-argument:** "Different domain names (`slug`/`screen_count` vs `id`/`name`/`requirement_count`) maintain a semantic boundary." Loses because the mapping function already collapses that boundary — the boundary is illusory, and the 28-line match is the cost of pretending otherwise.

**Depends on:** none.

---

### S2. FenceError: hand-rolled Display → thiserror

**Evidence:**

`src/commands/context/fences/parse.rs` defines `FenceError` (line 57) with a 33-line `impl std::fmt::Display for FenceError` (lines 79–112) and a 1-line `impl std::error::Error for FenceError {}` (line 115). Every Display arm is a fixed format string with at most one interpolated field — the exact shape `#[error("…")]` handles.

The root crate already depends on `thiserror` transitively via `specify-error`. Every other error enum in the workspace uses `thiserror::Error`.

**Action:**

1. Add `#[derive(thiserror::Error)]` to `FenceError` (replaces `impl std::error::Error`).
2. Add `#[error("…")]` attribute to each of the 9 variants, copying the string from the corresponding Display arm.
3. Delete `impl std::fmt::Display for FenceError` (lines 79–112, 34 lines) and `impl std::error::Error for FenceError {}` (line 115, 1 line).

Before:
```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::commands::context) enum FenceError {
    ExistingUnfencedAgentsMd,
    // …
}
impl std::fmt::Display for FenceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ExistingUnfencedAgentsMd => f.write_str("context-existing-unfenced…"),
            // 8 more arms
        }
    }
}
impl std::error::Error for FenceError {}
```

After:
```rust
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(in crate::commands::context) enum FenceError {
    #[error("context-existing-unfenced-agents-md: AGENTS.md exists without Specify context fences; rerun with --force to rewrite it")]
    ExistingUnfencedAgentsMd,
    // 8 more variants with #[error("…")]
}
```

**Quality delta:** −25 LOC, hand-rolled → idiomatic (ripgrep, helix, and every other thiserror consumer use this pattern).

**Net LOC:** parse.rs 341 → 316.

**Done when:** `rg 'impl std::fmt::Display for FenceError' src/` returns 0 hits.

**Rule?** No — this is the only hand-rolled Display+Error pair in the workspace (confirmed by grep).

**Counter-argument:** "`PartialEq` on a `thiserror::Error` type is unusual." Loses because `thiserror::Error` is compatible with `PartialEq` and tests already depend on it; the derive just generates the same code.

**Depends on:** none.

---

### S3. Collapse DiagnosticSeverity into Severity

**Evidence:**

`crates/domain/src/change/plan/core/model.rs` defines `Severity { Error, Warning }` (lines 180–187, no derives beyond Debug/Clone/PartialEq/Eq). `crates/domain/src/change/plan/doctor.rs` defines `DiagnosticSeverity { Error, Warning }` (lines 76–91) with Serialize, Deserialize, strum::Display, strum::IntoStaticStr, plus a 6-line `label()` method (lines 93–98) and an 8-line `From<&Severity>` impl (lines 101–108) that mechanically maps each variant to itself.

`DiagnosticSeverity` exists solely to carry serde/strum derives that `Severity` lacks. `crates/domain` already depends on serde and strum.

**Action:**

1. On `Severity` in model.rs, add `Copy, Serialize, Deserialize, strum::Display, strum::IntoStaticStr` derives plus `#[serde(rename_all = "kebab-case")]` and `#[strum(serialize_all = "kebab-case")]` attributes.
2. Delete `DiagnosticSeverity` enum, its `label()` impl, and the `From<&Severity>` impl from `doctor.rs` (−22 lines).
3. Replace all `DiagnosticSeverity` references (13 sites in `doctor.rs`, `cycle.rs`, `orphan_source.rs`, `stale_clone.rs`, `unreachable.rs`, `tests.rs`, and the re-export in `change.rs`) with `Severity`.

Before (`doctor.rs` lines 86–108):
```rust
pub enum DiagnosticSeverity { Error, Warning }
impl DiagnosticSeverity { pub fn label(self) -> &'static str { self.into() } }
impl From<&Severity> for DiagnosticSeverity { /* 2 arms */ }
```

After: deleted; `Severity` used directly.

**Quality delta:** −22 LOC, −1 type, −2 branches (the From match arms).

**Net LOC:** doctor.rs 248 → 226; model.rs 384 → 387 (+3 derive/attr lines).

**Done when:** `rg 'DiagnosticSeverity' crates/` returns 0 hits.

**Rule?** No.

**Counter-argument:** "Separate wire type insulates model.rs from serde churn." Loses because model.rs already derives Serialize+Deserialize on `Status` (line 22) via the same dependency — `Severity` is the unexplained holdout.

**Depends on:** none.

---

### S4. Delete ValidationRow, derive Serialize on Summary

**Evidence:**

`src/output.rs` defines `ValidationRow<'a>` (lines 208–215, 8 lines) and a `From<&'a ValidationSummary>` impl (lines 217–225, 9 lines). `ValidationRow` has identical field names and shapes to `ValidationSummary` (rule_id, rule, status, detail); the only difference is borrowed `&str` vs owned `String`. The type exists for Serialize — but `ValidationSummary` in `crates/error/src/validation.rs` already has `serde` available (Cargo.toml lists `serde.workspace = true`) and its sibling `Status` already derives `serde::Serialize`.

`ValidationRow` is used in `output.rs` (ErrorBody) and `src/commands/codex.rs` (ValidateBody).

**Action:**

1. In `crates/error/src/validation.rs`, add `serde::Serialize` to the `Summary` derive and add `#[serde(rename_all = "kebab-case")]`.
2. Delete `ValidationRow` struct, its `From` impl, and its doc block from `output.rs` (−18 lines).
3. In `output.rs`, change `ErrorBody.results` from `Option<Vec<ValidationRow<'a>>>` to `Option<&'a [ValidationSummary]>`. Simplify the `From<&'a Error>` impl's results branch from `results.iter().map(ValidationRow::from).collect()` to `Some(results.as_slice())` (−2 lines).
4. In `codex.rs`, change `ValidateBody.results` from `Vec<ValidationRow<'a>>` to `Vec<&'a ValidationSummary>`. Simplify the construction (−1 line). Remove `use crate::output::ValidationRow;` import.

**Quality delta:** −19 LOC, −1 type, −2 call-site ceremony (no more `.map(ValidationRow::from).collect()`).

**Net LOC:** output.rs 227 → 209; codex.rs ~−2; validation.rs +1.

**Done when:** `rg 'ValidationRow' src/` returns 0 hits.

**Rule?** No.

**Counter-argument:** "Borrowing &str avoids cloning in the JSON path." Loses because (a) this is the error path, serialized at most once per invocation, (b) `serde::Serialize` on `String` is zero-copy for the serializer (it borrows), and (c) the `collect()` allocation is strictly more expensive than a slice reference.

**Depends on:** none.

---

### S5. OutcomeKind: hand-rolled Display + discriminant → strum

**Evidence:**

`crates/domain/src/slice/outcome.rs` defines `Kind` (line 15) with `Serialize, Deserialize` and `#[serde(rename_all = "kebab-case")]`. It has a hand-rolled `impl fmt::Display for Kind` (lines 42–48, 7 lines) that delegates to `self.discriminant()`, and a `discriminant()` method (lines 50–61, 12 lines including doc) returning hardcoded kebab-case strings. The sibling `Phase` enum (same crate, `capability.rs` line 95) already uses `strum::Display` with `#[strum(serialize_all = "kebab-case")]` for the same purpose.

`discriminant()` is called at 2 external sites (`src/commands/slice/outcome.rs` lines 43, 182) as `x.discriminant().to_string()` — after adding strum, these become `x.to_string()`.

**Action:**

1. Add `strum::Display` to `Kind`'s derive list; add `#[strum(serialize_all = "kebab-case")]`.
2. Delete `impl fmt::Display for Kind` (lines 42–48) and the `discriminant()` method (lines 50–61).
3. Remove `use std::fmt;` import if no longer needed.
4. At the 2 call sites, replace `x.discriminant().to_string()` with `x.to_string()`.

Before (`outcome.rs` lines 42–61):
```rust
impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.discriminant())
    }
}
impl Kind {
    pub const fn discriminant(&self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Failure => "failure",
            Self::Deferred => "deferred",
            Self::RegistryAmendmentRequired { .. } => "registry-amendment-required",
        }
    }
}
```

After: deleted (strum derive handles it).

**Quality delta:** −14 LOC, hand-rolled → idiomatic (same pattern as `Phase` in this crate).

**Net LOC:** outcome.rs 82 → 68.

**Done when:** `rg 'fn discriminant' crates/domain/src/slice/outcome.rs` returns 0 hits.

**Rule?** No.

**Counter-argument:** "`discriminant()` is `const fn`, strum is not." Loses because no call site uses it in a const context, and the 2 external sites immediately call `.to_string()` which is non-const anyway.

**Depends on:** none.

---

## One-Touch Tidies

### T1. Inline single-use ToolError constructors

**Evidence:**

`crates/tool/src/error.rs` defines `manifest_read` (lines 167–173) and `manifest_parse` (lines 176–182). Each is called exactly once, in `crates/tool/src/load.rs` lines 50 and 54. No second call site exists (`rg 'manifest_read\|manifest_parse' crates/tool/src/` returns only the definition and one call each).

**Action:** Inline the struct construction at the call site in `load.rs`; delete both methods from `error.rs`.

Before (`load.rs:50`):
```rust
Err(err) => return Err(ToolError::manifest_read(sidecar_path, err)),
```
After:
```rust
Err(err) => return Err(ToolError::Manifest { path: sidecar_path, kind: ManifestKind::Read(err) }),
```
Same pattern for `manifest_parse` at line 54.

**Quality delta:** −12 LOC.

**Net LOC:** error.rs 260 → 248.

**Done when:** `rg 'fn manifest_read|fn manifest_parse' crates/tool/src/error.rs` returns 0 hits.

**Rule?** No.

**Counter-argument:** "Named constructors improve readability at the call site." Unfalsifiable — single-use helpers with no abstraction benefit are just indirection.

**Depends on:** none.

---

### T2. project.mdc H2 section cap contradiction

**Evidence:**

`specify/.cursor/rules/project.mdc` line 278 says "per-H2 section must stay under 45 lines" and line 279 immediately says "Each H2 section caps at 60 non-blank, non-comment lines." The normative source `docs/standards/skill-authoring.md` line 32 says "**Per-H2 section** ≤ **45 lines**" and line 36 says "The 200 / 45 / 512 numbers are kept synchronized."

**Action:** In `project.mdc`, delete line 279 (the "60" bullet).

**Quality delta:** −1 LOC, fixes an actively misleading instruction.

**Net LOC:** project.mdc 310 → 309.

**Done when:** `rg '60 non-blank' .cursor/rules/project.mdc` returns 0 hits (run from specify repo root).

**Rule?** No.

**Counter-argument:** "60 is the enforced cap in `checkSectionLineCount`." If true, either the check or the standard is wrong — either way, the two bullets must not say different numbers.

**Depends on:** none.

---

### T3. Skill prose: redundant generic instructions

**Evidence:**

Several SKILL.md files restate behaviors models follow by default:

- `plugins/client/skills/sow-writer/SKILL.md` ~line 154: "**Be thorough**; better to explicitly exclude…"
- `plugins/omnia/skills/test-writer/SKILL.md` ~line 48: "NEVER skip …" (generic vigilance framing around a substantive assertion rule)
- `plugins/vectis/skills/template-updater/SKILL.md` ~line 52: "Confirm missing SDKs **before editing**… must not … paper over …"

**Action:** Trim "Be thorough" and "NEVER skip" / "must not paper over" qualifiers to their substantive content. These are 1–2 line edits per skill, ~5 lines total across 3 skills.

**Quality delta:** −5 LOC (estimated), tightens skill body budget.

**Done when:** `rg -i 'be thorough' plugins/` returns 0 hits.

**Rule?** No — the count is 3, and the language is per-skill, not a systemic pattern.

**Counter-argument:** "Emphasis phrasing reinforces domain-specific constraints." Partially true for `test-writer` (the assertion rule is real); trim only the generic qualifier, keep the domain constraint.

**Depends on:** none.

---

### T4. `#[allow(dead_code)]` on test helper in finalize.rs

**Evidence:**

`crates/domain/tests/finalize.rs` line 940–941:
```rust
// silence unused-import warnings for Outcome — referenced in doctest-only
#[allow(dead_code)]
```

This suppresses a warning on a symbol only referenced in doctests. If the doctest is gone or the import is unused, delete the import and the allow. If the doctest remains, the allow is correct and this tidy is a no-op.

**Action:** Verify `Outcome` usage in the file; if no live reference exists, delete the import and the `#[allow(dead_code)]` attribute.

**Quality delta:** −2 LOC (conditional).

**Done when:** `rg 'allow(dead_code)' crates/domain/tests/finalize.rs` returns 0 hits.

**Rule?** No.

**Counter-argument:** "The doctest still references it." If so, the allow is correct and this tidy should be dropped.

**Depends on:** none.

---

### T5. Skill cross-references: paths → slash commands

**Evidence:**

5 SKILL.md files under `plugins/spec/skills/` reference sibling skills by filesystem path rather than slash command:

- `analyze/SKILL.md` lines 19, 44, 153, 164: `../extract/SKILL.md`, `../../../change/skills/plan/SKILL.md`
- `define/SKILL.md` lines 109, 128: `../analyze/SKILL.md`, `../../../change/skills/execute/SKILL.md`
- `extract/SKILL.md` line 19: `../analyze/SKILL.md`
- `drop/SKILL.md` line 17: `../../../change/skills/execute/SKILL.md`

The discovery model loads skills by slash command (`/spec:analyze`), not by filesystem path. Path-based links are fragile to moves and skip the discovery layer.

**Action:** Replace each `[text](../relative/SKILL.md)` with the slash-command form `\`/spec:analyze\`` (or `/change:execute`, etc.). ~10 edits across 4 files, 0 net LOC change per edit.

**Quality delta:** 0 LOC change, −5 fragile path references (reduces maintenance risk).

**Done when:** `rg 'SKILL\.md' plugins/spec/skills/*/SKILL.md` returns only self-references (the stop-reading directive).

**Rule?** No — 5 hits is close to the 3× threshold but the fix is per-file, not scriptable.

**Counter-argument:** "Path links are clickable in the IDE." True, but the skill body is consumed by agents, not IDE users — agents resolve `/spec:analyze`, not `../analyze/SKILL.md`.

**Depends on:** none.

---

## Post-mortem

| Item | Predicted ΔLOC | Actual ΔLOC | "Done when" clean? | Regressions |
|---|---|---|---|---|
| S1 | −55 | −62 | Yes — `rg '\bMergeOp\b' crates/domain/src/merge/composition.rs` returns 0 hits | None — 43 composition/merge tests pass, clippy clean |
| S2 | −25 | −24 | Yes — `rg 'impl std::fmt::Display for FenceError' src/` returns 0 hits | None — full `cargo make ci` passes; added `thiserror.workspace = true` to root crate Cargo.toml (+1 line not predicted) |
| S3 | −22 | −23 | Yes — `rg 'DiagnosticSeverity' crates/` returns 0 hits | None — full `cargo make ci` passes (627 tests, clippy, docs, fmt, vet, deny). Extra −1 from `PlanDoctorSeverity` re-export removal reformatting `change.rs` import block. Added `label()` method to `Severity` to preserve wire-layer call site in `src/commands/change/plan/doctor.rs`. |
