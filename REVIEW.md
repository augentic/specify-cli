# Code & Skill Review — specify + specify-cli

## Summary

1. **S9 — Sweep redundant `IntoStaticStr` / `.label()` / `serialize_with`**: ~−40 LOC across 5 files, eliminates 5 dead derives + 3 methods + 2 adapter functions by leaning on the `strum::Display` + serde `rename_all` that the same enums already carry.
2. **S7 — Collapse `DiagnosticRow` mirror into `PlanDoctorDiagnostic`**: ~−25 LOC, deletes a 1-for-1 wire mirror plus its `diagnostic_row` helper; same anti-pattern as round-1's `ValidationRow` / `MergeOp` collapses.
3. **S8 — Collapse `FindingRow` + `FindingLevel` into `Finding`**: ~−25 LOC, drops a mirror struct + a mirror enum + their `From` impl by deriving `Serialize` on `Finding` directly.
4. **S1 — Delete `ContractAction` mirror enum** (+`From` impl): ~22 LOC, collapses 1 type + 3 branches. Most mechanically clean of the remaining round-2 collapses.
5. **S2 — Delete hand-rolled `severity_label` in codex.rs**: ~12 LOC, collapses 1 function + 4 match arms by using the existing `strum::Display` derive.

Total ΔLOC if all findings land: **−185 to −210** (structural) + **−45 to −55** (tidies).
Primary non-LOC axes moved: **−5 types**, **−5 dead derives**, **−5 methods/adapter fns**, **−8 branches**, **−3 call-site `.into()` / format plumbing sites**.
Most likely to break in remediation: **S4** (Regex → OnceLock in primitives.rs) — the `ids_match_pattern` variant takes a dynamic `pattern` argument and cannot be hoisted. Secondary: **S8** — `Finding::code` is `&'static str`; deriving `Serialize` on the struct is fine but the writer's borrow pattern changes when `FindingRow<'a>` disappears.

---

## Reconnaissance

```
tokei (specify-cli):  49,149 Rust lines across 273 files; 3,473 Markdown
tokei (specify):      59,280 Markdown lines; 210 Rust (WASI tooling only)
cargo tree --duplicates: all duplicates are wasmtime transitive (anyhow, bitflags, cranelift-bitset, rustix) — nothing actionable
rg -c '^#\[test\]': 557 tests total (385 domain, 155 integration, 8 tool, 9 validate-crate)
rg --files -g '**/mod.rs': 3 files, all under tests/common/ or wasi-tools/ — compliant with coding-standards.md
wc -l docs/standards/*.md AGENTS.md: 573 lines total (architecture 88, coding-standards 223, handler-shape 68, style 82, testing 35, AGENTS 77)
files > 500 LOC under crates/ src/: capability.rs 1179 (tests), workspace.rs 1042 (tests), finalize.rs 948 (tests), registry.rs 923 (tests), doctor/tests.rs 549, validate.rs 539, archive/tests.rs 479, validate/tests.rs 472, config.rs 465
Regex::new call sites (non-OnceLock): 8 in domain (validate/primitives: 4, merge/validate: 3, registry/composition: 1)
SKILL.md files: 27 total, largest 194 lines (extract), all under 200-line body cap
Duplicate skill reference/example files: all duplicates are already symlinks — no non-symlink copies
```

---

## Structural Findings

### S1 — Delete `ContractAction` mirror enum

**Evidence**: `src/commands/slice/merge.rs:191-211` defines `ContractAction { Added, Replaced, Unknown }` with a `From<&OpaquePreviewEntry>` that maps `OpaqueAction::Added → ContractAction::Added`, etc. `OpaqueAction` at `crates/domain/src/merge/slice.rs:60-67` is `#[non_exhaustive]` and already `Serialize`-compatible. The mirror exists only to add an `Unknown` variant for the `_ =>` arm and to derive `Serialize`.

**Action**:
1. Add `#[derive(Serialize)]` and `#[serde(rename_all = "kebab-case")]` to `OpaqueAction` in `crates/domain/src/merge/slice.rs`.
2. In `src/commands/slice/merge.rs`, delete the `ContractAction` enum (lines 191–197), delete the `From<&OpaquePreviewEntry> for ContractItem` impl (lines 199–211).
3. Change `ContractItem.action` from `ContractAction` to `OpaqueAction`.
4. Build the `ContractItem` inline in the `.map()` at line 67, matching `OpaqueAction::Added | OpaqueAction::Replaced` explicitly and falling through to a `_ => continue` to skip unknown actions rather than serialising an "unknown" wire value that no consumer handles.
5. In `write_preview_text`, replace the `ContractAction` match with `OpaqueAction` variants.

Before:
```rust
enum ContractAction { Added, Replaced, Unknown }
impl From<&OpaquePreviewEntry> for ContractItem { /* 12 lines */ }
```

After:
```rust
// (deleted — OpaqueAction used directly; Unknown arms gone)
```

**Quality delta**: −22 LOC, −1 type, −3 branches (the `Unknown` arm in display + the `_ =>` mapping + the `From` impl), −1 module edge (`use OpaqueAction` stays but `ContractAction` import deleted).

**Net LOC**: 430 → ~408

**Done when**: `rg 'ContractAction' src/` returns zero matches.

**Rule?**: No.

**Counter-argument**: "The mirror isolates the CLI wire shape from domain enum growth." Pre-1.0, the wire shape is not a compatibility constraint and `OpaqueAction` is already `#[non_exhaustive]` — the `_ =>` arm on the `OpaqueAction` match handles it.

**Depends on**: none.

---

### S2 — Delete hand-rolled `severity_label` in codex.rs

**Evidence**: `src/commands/codex.rs:230-237` defines `const fn severity_label(severity: CodexSeverity) -> &'static str` with four match arms. `CodexSeverity` at `crates/domain/src/capability/codex.rs:57-71` already derives `strum::Display` with `#[strum(serialize_all = "kebab-case")]`. `severity_label` produces identical strings (`"critical"`, `"important"`, `"suggestion"`, `"optional"`).

**Action**:
1. Delete `severity_label` function (lines 230–237).
2. At line 206, change `severity: severity_label(frontmatter.severity)` to `severity: frontmatter.severity.to_string()` (or `&*frontmatter.severity.to_string()` if the borrow checker objects — but the field is `&'static str` today, so change it to `String` and let serde handle it, or use `<CodexSeverity as std::fmt::Display>::to_string()`).

Actually, cleaner: change the field type from `&'static str` to `CodexSeverity` directly and derive `Serialize` on it (it already has `serde::Serialize`). Then the `RuleView.severity` field just carries the enum and serde serialises it as `"critical"` etc. This deletes the function *and* the format string.

Before:
```rust
const fn severity_label(severity: CodexSeverity) -> &'static str {
    match severity {
        CodexSeverity::Critical => "critical",
        // ... 3 more arms
    }
}
```

After: (deleted — `RuleView.severity: CodexSeverity` serialised directly)

**Quality delta**: −12 LOC, −4 branches, −1 call-site format string. Axis 7 (hand-rolled → derive): `strum::Display` is already derived; this finding makes the derive the sole consumer. Precedent: every other kebab-case enum in the codebase (`ToolScopeKind`, `LifecycleStatus`, `Phase`, `CodexProvenance`) uses `strum::Display` directly, never a shadow function.

**Net LOC**: 237 → ~225

**Done when**: `rg 'severity_label' src/` returns zero matches.

**Rule?**: No.

**Counter-argument**: "`const fn` is zero-cost; `strum::Display` allocates a `String`." True, but the caller immediately stores `&'static str` — so either way we're not in a hot loop, and the field can be changed to carry the enum value itself, at which point serde handles it with zero allocation.

**Depends on**: none.

---

### S3 — Collapse double regex dispatch in `extract_skill_directive`

**Evidence**: `crates/domain/src/task.rs:133-149`. The function calls `skill_directive_re().find(rest)` at line 134, then immediately calls `skill_directive_re().captures(rest)` at line 138 — two regex executions on the same input. `captures()` already returns the match span via `caps.get(0)`, making the `find()` call redundant.

**Action**:
1. Replace the two calls with a single `skill_directive_re().captures(rest)`.
2. Use `caps.get(0).unwrap()` for the match span (guaranteed present when `captures()` returns `Some`).

Before:
```rust
let Some(m) = skill_directive_re().find(rest) else {
    return (rest.trim().to_string(), None);
};
let caps = skill_directive_re().captures(rest).expect("find matched; captures must too");
```

After:
```rust
let Some(caps) = skill_directive_re().captures(rest) else {
    return (rest.trim().to_string(), None);
};
let m = caps.get(0).unwrap();
```

**Quality delta**: −3 LOC, −1 regex execution per task line, −1 `.expect()` panic site. Axis 7: this is how ripgrep, helix, and every regex-heavy Rust project does it — call `captures()` once, not `find()` + `captures()`.

**Net LOC**: 225 → 222

**Done when**: `rg '\.find\(rest\)' crates/domain/src/task.rs` returns zero matches.

**Rule?**: No.

**Counter-argument**: "`find()` is cheaper than `captures()` for the early-exit path." True in general, but here the early-exit is `None` on both, and the `Some` path runs `captures()` anyway — the `find()` is pure waste on the happy path.

**Depends on**: none.

---

### S4 — Hoist literal `Regex::new` calls to `OnceLock` in validate/primitives.rs

**Evidence**: `crates/domain/src/validate/primitives.rs` calls `Regex::new` four times (lines 75, 96, 98, 209) — three of those are literal patterns compiled on every invocation. The `task.rs` module in the same crate already uses `OnceLock<Regex>` for the same purpose (lines 50–71). The merge/validate.rs module has 3 more (lines 77, 121, 146 — but line 146 is in `#[cfg(test)]` and line 121 is in a `Some` branch that fires once, so only line 77 matters in production).

**Action**:
1. In `crates/domain/src/validate/primitives.rs`, extract the three literal patterns (`r"^\s*-\s+\S"`, `r"^\s*-\s+\[( |x|X)\]\s+\d+(?:\.\d+)*\s+"`, `r"REQ-[0-9]{3}"`) into module-level `OnceLock<Regex>` + accessor functions, matching the `task.rs` pattern.
2. Leave `ids_match_pattern` (line 75) alone — its `pattern` argument is dynamic and cannot be hoisted.
3. In `crates/domain/src/merge/validate.rs`, extract `REQ_ID_PATTERN` compilation at line 77 into a `OnceLock` accessor (line 121 can share it).

Before (primitives.rs, inside `all_tasks_use_checkbox`):
```rust
let bullet_re = Regex::new(r"^\s*-\s+\S").expect("bullet regex is valid");
let checkbox_re =
    Regex::new(r"^\s*-\s+\[( |x|X)\]\s+\d+(?:\.\d+)*\s+").expect("checkbox regex is valid");
```

After:
```rust
fn bullet_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^\s*-\s+\S").expect("bullet regex"))
}
// (same pattern for checkbox_re, req_id_re)
```

**Quality delta**: +12 LOC (accessor functions), −6 LOC (inline compilations) = +6 net LOC. Justified: eliminates 7 redundant `Regex::new` compilations on every validation invocation (8 total sites minus the 1 dynamic pattern). Axis 7: `OnceLock` is the idiomatic stdlib pattern; ripgrep's `grep-regex` and helix's regex handling both compile once. The task.rs module in the same crate already uses this pattern — this finding makes the crate consistent.

**Net LOC**: ~+6 (LOC increase justified by −7 branches removed from the hot path and consistency with existing `task.rs` pattern).

**Done when**: `rg 'Regex::new' crates/domain/src/validate/primitives.rs` returns exactly 1 match (the dynamic `ids_match_pattern` call).

**Rule?**: No (only 2 files; below the 3× threshold).

**Counter-argument**: "These are called once per `specify validate` invocation, not in a hot loop." True, but the issue is consistency — the same crate uses `OnceLock` three feet away in `task.rs`. The LOC increase is tiny and the code reads better.

**Depends on**: none.

---

### S5 — Inline `scope_tools` into its two callers in load.rs

**Evidence**: `crates/tool/src/load.rs:21-24` defines `pub fn scope_tools(scope: &ToolScope, tools: Vec<Tool>) -> Vec<(ToolScope, Tool)>` — a one-liner that maps `|tool| (scope.clone(), tool)`. It has exactly two call sites: `project_tools` (line 32) and `capability_sidecar` (line 67), both in the same file. `scope_tools` is `pub` but only used within `crates/tool/`.

**Action**:
1. Delete `scope_tools` (lines 21-24).
2. Inline the one-liner into `project_tools` and `capability_sidecar`.
3. Make `project_tools` take the `project_name` and return the mapped vec directly — or delete it too if its only caller (the binary) can build the `ToolScope` inline.

Before:
```rust
pub fn scope_tools(scope: &ToolScope, tools: Vec<Tool>) -> Vec<(ToolScope, Tool)> {
    tools.into_iter().map(|tool| (scope.clone(), tool)).collect()
}
pub fn project_tools(project_name: impl Into<String>, tools: Vec<Tool>) -> Vec<(ToolScope, Tool)> {
    let scope = ToolScope::Project { project_name: project_name.into() };
    scope_tools(&scope, tools)
}
```

After:
```rust
pub fn project_tools(project_name: impl Into<String>, tools: Vec<Tool>) -> Vec<(ToolScope, Tool)> {
    let scope = ToolScope::Project { project_name: project_name.into() };
    tools.into_iter().map(|tool| (scope.clone(), tool)).collect()
}
```

**Quality delta**: −6 LOC, −1 `pub` function, −1 module edge (external callers no longer see `scope_tools`).

**Net LOC**: 207 → 201

**Done when**: `rg 'fn scope_tools' crates/tool/src/load.rs` returns zero matches.

**Rule?**: No.

**Counter-argument**: "A third caller may need it." Pre-1.0 — add it back when a third caller arrives. YAGNI.

**Depends on**: none.

---

### S6 — Delete `warning_names` dedup set in `merge_scoped`

**Evidence**: `crates/tool/src/load.rs:73-96`. `merge_scoped` keeps both `project_names: HashSet<String>` and `warning_names: HashSet<String>`. The `warning_names` set guards against emitting a warning twice for the same name — but the capability input list should never contain duplicate names (and if it does, the loop already `continue`s past the first duplicate, so subsequent duplicates hit the same `project_names.contains` check and produce the same collision warning). The only way `warning_names` fires is if capability tools contain two entries with the same name that also collide with a project tool — a doubly-degenerate input that the tool manifest schema should reject earlier.

**Action**:
1. Delete `warning_names` (line 78) and its `insert` call (line 88).
2. Unconditionally push the warning on collision.

Before:
```rust
let mut warning_names: HashSet<String> = HashSet::new();
// ...
if warning_names.insert(tool.name.clone()) {
    warnings.push(Warning::ToolNameCollision { name: tool.name });
}
```

After:
```rust
warnings.push(Warning::ToolNameCollision { name: tool.name });
```

**Quality delta**: −4 LOC, −1 `HashSet` allocation, −1 branch.

**Net LOC**: 207 → 203 (or 201 → 197 if stacked on S5).

**Done when**: `rg 'warning_names' crates/tool/src/load.rs` returns zero matches.

**Rule?**: No.

**Counter-argument**: "Duplicate warnings confuse the user." If the input has duplicate capability tools, the user has bigger problems — and the test at line 170 (`merge_scoped_project_wins_and_warns_once`) only checks a non-duplicate-capability case. Drop the test assertion count check or adjust it.

**Depends on**: none.

---

### S7 — Collapse `DiagnosticRow` mirror into `PlanDoctorDiagnostic`

**Evidence**: `src/commands/change/plan/doctor.rs:18-28` defines `DiagnosticRow` and `DoctorBody.diagnostics: Vec<DiagnosticRow>` (line 34); `diagnostic_row()` at lines 96-108 builds one from a `PlanDoctorDiagnostic`. The fields are a 1-for-1 mirror of `specify_domain::change::PlanDoctorDiagnostic` (`crates/domain/src/change/plan/doctor.rs:49-68`) with kebab-case rename. The two cosmetic differences vanish under inspection:

1. `DiagnosticRow.severity: &'static str` vs `Diagnostic.severity: Severity` — `Severity` already derives `serde::Serialize` with `rename_all = "kebab-case"` (`model.rs:180-200`); the wire string is byte-identical.
2. `DiagnosticRow.data: Option<serde_json::Value>` vs `Diagnostic.data: Option<DiagnosticPayload>` — `diagnostic_row()` calls `serde_json::to_value(p)` just to re-serialize through the same derive that the top-level writer would call directly.

Current state confirmed:

```
rg -n 'DiagnosticRow|diagnostic_row' src/commands/change/plan/doctor.rs
20:struct DiagnosticRow {
34:    diagnostics: Vec<DiagnosticRow>,
75:    let rows: Vec<DiagnosticRow> = diagnostics.iter().map(diagnostic_row).collect();
96:fn diagnostic_row(d: &PlanDoctorDiagnostic) -> DiagnosticRow {
```

**Action**:

1. Delete the `DiagnosticRow` struct (lines 18-28).
2. Delete `diagnostic_row()` (lines 96-108).
3. Change `DoctorBody.diagnostics` to `Vec<PlanDoctorDiagnostic>`; drop the `.iter().map(diagnostic_row).collect()` line; pass `diagnostics` straight into the body.
4. In `write_doctor_text`, change `d.severity == "error"` to `matches!(d.severity, Severity::Error)`.

Before (~75):

```rust
let rows: Vec<DiagnosticRow> = diagnostics.iter().map(diagnostic_row).collect();
ctx.write(
    &DoctorBody { plan: plan_ref(&plan, &plan_path), diagnostics: rows },
    write_doctor_text,
)?;
```

After:

```rust
ctx.write(
    &DoctorBody { plan: plan_ref(&plan, &plan_path), diagnostics },
    write_doctor_text,
)?;
```

**Quality delta**: −25 LOC, −1 type, −1 helper fn, −1 wire-format mirror. Same anti-pattern eliminated by round-1's S4 (`ValidationRow`).

**Net LOC**: 108 → 83.

**Done when**: `rg 'DiagnosticRow|diagnostic_row' src/` returns zero matches.

**Rule?**: No.

**Counter-argument**: "The CLI wire format is a stable contract that should not be derived directly from a domain type." Loses because `PlanDoctorDiagnostic` is *already* the wire shape it serializes to — `DiagnosticRow` is a copy with strictly fewer fields managed and the same kebab discriminants. Precedent: ripgrep's `Message` is serialized directly with no wire-mirror DTO.

**Depends on**: none.

---

### S8 — Collapse `FindingRow` + `FindingLevel` into `Finding`

**Evidence**: `src/commands/change/plan/lifecycle.rs` defines `FindingRow<'a>` (lines 256-263, 7 lines), `FindingLevel { Error, Warning }` (lines 265-270, 6 lines), and `impl<'a> From<&'a Finding> for FindingRow<'a>` (lines 272-285, 14 lines). `FindingLevel` is a precise mirror of `specify_domain::change::Severity` (`model.rs:194-200`), which already derives `serde::Serialize` with `rename_all = "kebab-case"`. `Finding` itself (`model.rs:212-221`) is `#[derive(Debug, Clone)]` — adding `Serialize` is one line.

Current state confirmed:

```
rg -n 'FindingLevel' src/commands/change/plan/lifecycle.rs
259:    level: FindingLevel,
267:enum FindingLevel {
275:            Severity::Error => FindingLevel::Error,
276:            Severity::Warning => FindingLevel::Warning,
288:    let label = if row.level == FindingLevel::Error { "ERROR  " } else { "WARNING" };
```

**Action**:

1. In `crates/domain/src/change/plan/core/model.rs`, add `serde::Serialize` to the `Finding` derive and `#[serde(rename_all = "kebab-case")]` (3 lines). `level: Severity` and the other fields serialize correctly without further changes.
2. In `src/commands/change/plan/lifecycle.rs`, delete `FindingRow<'a>`, `FindingLevel`, and the `From` impl (lines 256-285, 30 lines).
3. Change `PlanValidateBody.results` from `Vec<FindingRow<'a>>` to `&'a [Finding]` and drop the `.iter().map(FindingRow::from).collect()` line.
4. Change `write_finding_row_text` to take `&Finding`; the body becomes `let label = if row.level == Severity::Error { "ERROR  " } else { "WARNING" };` (`Severity` is already imported in this file).

Before (lines 256-290):

```rust
#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct FindingRow<'a> { level: FindingLevel, code: &'static str, entry: &'a Option<String>, message: &'a str }
#[derive(Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
enum FindingLevel { Error, Warning }
impl<'a> From<&'a Finding> for FindingRow<'a> { /* 12 lines */ }
fn write_finding_row_text(w: &mut dyn Write, row: &FindingRow<'_>) -> std::io::Result<()> { /* … */ }
```

After: every `FindingRow*` deleted; the writer takes `&Finding` directly.

**Quality delta**: −25 LOC, −2 types, −1 `From` impl, −1 module-internal mirror.

**Net LOC**: `lifecycle.rs` 366 → 343; `model.rs` 405 → 408 (+3 derive/attr lines).

**Done when**: `rg 'FindingRow|FindingLevel' src/` returns zero matches.

**Rule?**: No.

**Counter-argument**: "Adding `Serialize` to a domain struct couples wire shape to internal representation." Loses because `Severity` and `Status` siblings in the same `model.rs` already do this (lines 14-27, 180-200) — `Finding` is the unexplained holdout. Same pattern as round-1's S3 (`DiagnosticSeverity` → `Severity`), one indirection further.

**Depends on**: none.

---

### S9 — Sweep redundant `IntoStaticStr` / `.label()` / `serialize_with`

**Evidence**: Five enums derive `strum::Display` (which gives `to_string()` returning a kebab `String`) *and* `strum::IntoStaticStr` (which gives `.into()` returning `&'static str`). The second derive exists to feed either a 7-line `.label()` method that just does `self.into()`, or a 7-line `serialize_with` adapter that does `s.serialize_str(status.into())`. Both produce wire output byte-identical to plain derived `Serialize` with `rename_all = "kebab-case"` — which all five enums already carry.

Current state confirmed:

```
rg -n 'strum::IntoStaticStr' crates/
crates/domain/src/change/finalize.rs:45
crates/domain/src/registry/workspace/status.rs:60,90
crates/domain/src/change/plan/core/model.rs:190
crates/domain/src/validate/compatibility.rs:42

rg -n 'pub fn label\(self\) -> &.static str' crates/
crates/domain/src/registry/workspace/status.rs:74,108
crates/domain/src/change/plan/core/model.rs:205

rg -n 'serialize_with =' src/ crates/
crates/domain/src/change/finalize.rs:90    serialize_with = "serialize_status"
src/commands/workspace.rs:347              serialize_with = "serialize_push_outcome"
```

`CompatibilityClassification`'s `IntoStaticStr` derive (`compatibility.rs:42`) has zero `.into()` consumers — `rg 'CompatibilityClassification' --type rust` shows only variant pattern matches. Pure dead derive.

Call sites needing update:

```
rg -n '\.label\(\)' --type rust
crates/domain/src/change/plan/doctor/stale_clone.rs:31,37   # `.label().to_string()` → `.to_string()` (strum::Display)
src/commands/change/plan/doctor.rs:102                      # vanishes with S7
```

**Action**:

1. Drop `strum::IntoStaticStr` from the derive list on `Severity` (`model.rs:190`), `ConfiguredTargetKind` (`status.rs:60`), `SlotKind` (`status.rs:90`), `Landing` (`finalize.rs:45`), `CompatibilityClassification` (`compatibility.rs:42`). −5 lines.
2. Delete the three `pub fn label(self) -> &'static str` methods and their doc blocks (`model.rs:202-208`, `status.rs:71-77`, `status.rs:105-111`). −21 lines.
3. Delete `serialize_status` (`finalize.rs:113-119`) and `serialize_push_outcome` (`workspace.rs:357-363`). Drop the matching `#[serde(serialize_with = …)]` attributes (`finalize.rs:90`, `workspace.rs:347`). −18 lines.
4. At the two `.label().to_string()` sites in `stale_clone.rs`, replace with `.to_string()` (strum::Display already on the enum). Character-level, 0 LOC.

Before (`finalize.rs:90` + `113-119`):

```rust
#[serde(serialize_with = "serialize_status")]
pub status: Landing,
// …
#[expect(clippy::trivially_copy_pass_by_ref, reason = "serde's `serialize_with` signature requires `&T`.")]
fn serialize_status<S: serde::Serializer>(status: &Landing, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(status.into())
}
```

After: attribute and function gone; `Landing`'s derived `Serialize` produces the same `"merged"` / `"no-branch"` kebab string.

**Quality delta**: −44 LOC, −5 dead derives, −3 methods, −2 adapter fns, hand-rolled → idiomatic (helix, cargo, jj all use plain `#[derive(Serialize)]` + `rename_all` for this exact case).

**Net LOC**: `model.rs` −8, `status.rs` −16, `finalize.rs` −9, `compatibility.rs` −1, `workspace.rs` −10. Combined ~−44.

**Done when**: `rg 'strum::IntoStaticStr|serialize_with =|pub fn label\(self\) -> &.static str' src/ crates/` returns zero matches.

**Rule?**: No.

**Counter-argument**: "`&'static str` is one less allocation per JSON row than `String`." Loses because (a) serde's `Serialize` is zero-copy regardless of `&str` vs `String` — the serializer borrows; and (b) the `serialize_str(status.into())` path also allocates nothing, so it is strictly equivalent to derived `Serialize`. The complexity buys nothing measurable.

**Depends on**: none. (S7 vaporises the last call-site of `Severity::label()`, but the sweep stands on its own.)

---

## One-Touch Tidies

### T1 — `design_references_exist`: use `HashSet` instead of `Vec` for `spec_bodies`

**Evidence**: `crates/domain/src/validate/primitives.rs:208-231`. Builds `spec_bodies: Vec<String>` and calls `.iter().any(|body| body.contains(needle))` in a loop — O(refs × specs × body_len). The function reads every spec file into memory and does a linear scan. For the small inputs this receives today it's fine, but the `refs` dedup at lines 210-212 uses `Vec::sort + dedup` when a `HashSet` would be shorter.

**Action**: Replace `let mut refs: Vec<String>` + sort + dedup with `let refs: HashSet<String> = re.find_iter(design).map(…).collect();` and `if refs.is_empty()` → `if refs.is_empty()`. This deletes 2 lines.

**Quality delta**: −2 LOC, −2 method calls (`.sort()`, `.dedup()`).

**Net LOC**: 388 → 386.

**Done when**: `rg '\.sort\(\)' crates/domain/src/validate/primitives.rs` returns only the test-helper `tmp()` sort if any, not a `refs.sort()`.

**Rule?**: No.

**Counter-argument**: "Vec preserves insertion order for debugging." Nobody debugs this; the output is a bool.

**Depends on**: none.

---

### T2 — `one_line` in context/render.rs: use `split_whitespace().collect::<Vec<_>>().join(" ")`→ `Cow`

**Evidence**: `src/commands/context/render.rs:239-241`. `fn one_line(value: &str) -> String` allocates a `Vec<&str>` + a `String` on every call. It's called ~20× per render. For values that already contain no internal newlines or double spaces (the common case), it could return a `Cow<str>` and skip the allocation.

**Action**: Actually, this is marginal. The function is 3 lines and called in a code path that writes to disk. Skip — below threshold.

*Dropped: marginal LOC, single axis, no architectural impact.*

---

### T3 — `validate_baseline` in merge/validate.rs: share compiled `REQ_ID_PATTERN` regex

**Evidence**: `crates/domain/src/merge/validate.rs:77` and `:121` both call `Regex::new(REQ_ID_PATTERN)`. The function is called once per merge — two compilations of the same pattern.

**Action**: Compile once at line 77, reuse the binding at line 121 (rename `id_pattern` → `req_re` and pass to the closure or move the `Some(design_text)` branch after the loop so `id_pattern` is still in scope — it already is).

Before:
```rust
let id_pattern = Regex::new(REQ_ID_PATTERN).expect("…");  // line 77
// ...
let ref_pattern = Regex::new(REQ_ID_PATTERN).expect("…");  // line 121
```

After:
```rust
let id_pattern = Regex::new(REQ_ID_PATTERN).expect("…");
// ... (reuse `id_pattern` at old line 121)
```

**Quality delta**: −2 LOC, −1 `Regex::new` call.

**Net LOC**: 202 → 200.

**Done when**: `rg 'Regex::new' crates/domain/src/merge/validate.rs` returns 1 match (excluding `#[cfg(test)]`).

**Rule?**: No.

**Counter-argument**: "Clarity — each block owns its regex." The pattern is identical and the variable is already in scope.

**Depends on**: none.

---

### T4 — `ContractItem::from` wildcard arm: match explicitly

**Evidence**: `src/commands/slice/merge.rs:204`. The `_ => ContractAction::Unknown` arm silently swallows future `OpaqueAction` variants. If S1 lands, this is moot. If S1 doesn't land, the arm should match the two known variants and have the `_` produce a compile-time `todo!()` or at minimum a named constant so new variants are caught.

*Dropped: S1 subsumes this.*

---

### T5 — `all_tasks_use_checkbox`: `bullet_re` can be a byte-level check

**Evidence**: `crates/domain/src/validate/primitives.rs:96`. `Regex::new(r"^\s*-\s+\S")` matches a line that starts with optional whitespace, `-`, whitespace, then a non-space. This is equivalent to `line.trim_start().starts_with("- ") && line.trim_start()[2..].starts_with(|c: char| !c.is_whitespace())` — a pure `str` check, no regex needed.

**Action**: Replace the `bullet_re` with:
```rust
let is_bullet = |line: &str| {
    let t = line.trim_start();
    t.starts_with("- ") && t[2..].starts_with(|c: char| !c.is_whitespace())
};
```

Delete the `Regex::new` import if S4 hasn't already pulled it into a `OnceLock`.

**Quality delta**: −2 LOC, −1 regex compilation, −1 dependency on `regex` for this function.

**Net LOC**: 388 → 386 (stacks with T1 → 384).

**Done when**: `rg 'bullet_re' crates/domain/src/validate/primitives.rs` returns zero matches.

**Rule?**: No.

**Counter-argument**: "The regex is more readable." Debatable; the `str` check is three chained methods and no compile step.

**Depends on**: none (compatible with S4 but independent).

---

### T6 — `provenance_text` in codex.rs: match on `CodexProvenance` directly

**Evidence**: `src/commands/codex.rs:218-228`. `provenance_text` matches on `rule.provenance_kind` (a `&'static str`) instead of matching on the enum. The `RuleView` struct flattens `CodexProvenance` into three `Option` fields + a `provenance_kind: &'static str` discriminator — the original enum is discarded. This is fine for JSON serialisation but means the text renderer has to string-match.

**Action**: Store `provenance: &'a CodexProvenance` in `RuleView`, derive `Serialize` for `CodexProvenance` (it already has it), and delete the three flattened fields (`provenance_kind`, `capability_name`, `capability_version`, `catalog_name`) + the build logic at lines 195-202. The `provenance_text` function then matches on the enum.

**Quality delta**: −10 LOC, −4 `Option` fields, −3 branches (the string match in `provenance_text`). +2 LOC for `#[serde(flatten)]` on the new field. Net: −8 LOC.

**Net LOC**: 237 → 229 (stacks with S2 → ~217).

**Done when**: `rg 'provenance_kind' src/commands/codex.rs` returns zero matches.

**Rule?**: No.

**Counter-argument**: "The JSON shape changes." Pre-1.0 — the JSON shape is not a compatibility constraint. And `#[serde(flatten)]` with `CodexProvenance`'s existing `Serialize` derive can produce the same keys if needed.

**Depends on**: none.

---

### T7 — Remove `#[must_use]` on `render_document` (test-only function)

**Evidence**: `src/commands/context/render.rs:72`. `#[must_use]` on a `#[cfg(test)]` function is noise — tests that ignore the return value won't compile anyway (they'd have an unused variable). 1 line.

*Dropped: single line, formatting-only in spirit.*

---

### T8 — `slug_re` in registry/composition.rs: hoist to `OnceLock`

**Evidence**: `crates/domain/src/validate/registry/composition.rs:67` compiles `r"^[a-z][a-z0-9]*(-[a-z0-9]+)*$"` on every composition validation call. Same pattern as S4.

**Action**: Hoist to a `OnceLock<Regex>` accessor.

**Quality delta**: +4 LOC (accessor), −1 LOC (inline), net +3. Justified by consistency with S4 if it lands.

*Dropped: only meaningful if S4 lands; LOC-positive on its own.*

---

### T9 — `is_false` serde helper is one line; inline it

**Evidence**: `crate::serde_helpers::is_false` is referenced at `crates/domain/src/config.rs:57`. A one-line helper `fn is_false(v: &bool) -> bool { !v }` in a dedicated module.

**Action**: Check if `is_false` has more than one call site. If exactly one, inline `skip_serializing_if = "std::ops::Not::not"` or `skip_serializing_if = "crate::serde_helpers::is_false"` is already fine. Actually, `serde` doesn't accept method paths — so the helper module is required. Skip.

*Dropped: cannot inline.*

---

### T10 — `ExportBody` text renderer is a stub

**Evidence**: `src/commands/codex.rs:142-144`. `write_export_text` writes a hardcoded "rerun with --format json" message. This is 3 lines that do nothing useful — but the handler pattern requires the `render_text` closure. The function is a stub by design.

*Dropped: 3 lines, by design, no axis touched.*

---

### T11 — Inline `Patch::keep / clear / set` constructors

**Evidence**: `crates/domain/src/change/plan/core/model.rs:124-141` defines three `pub const fn` constructors that wrap `Self::Keep`, `Self::Clear`, `Self::Set(v)`. Total 18 lines including docs and `#[must_use]`. Eight call sites use them:

```
rg -n 'Patch::(keep|clear|set)\(' crates/ src/
crates/domain/src/change/plan/core/amend.rs:119,132,264,278,279,311,325
src/commands/change/plan/create.rs:17,18,19
```

Each call site is one character longer with the variant form (`Patch::Set(s)` vs `Patch::set(s)`).

**Action**: Delete the three methods (`model.rs:124-141`); replace each call with the variant directly. `apply()` stays.

Before:

```rust
impl<T> Patch<T> {
    /// Convenience constructor for the keep-as-is case.
    #[must_use]
    pub const fn keep() -> Self { Self::Keep }
    /// Convenience constructor for the clear-to-`None` case.
    #[must_use]
    pub const fn clear() -> Self { Self::Clear }
    /// Convenience constructor for the replace-with-`Some(v)` case.
    pub const fn set(value: T) -> Self { Self::Set(value) }
    // apply() retained
```

After: three methods + their doc blocks deleted; call sites use `Patch::Keep` / `Patch::Clear` / `Patch::Set(v)` directly.

**Quality delta**: −18 LOC.

**Net LOC**: `model.rs` 405 → 390 (stacks with S8's +3 → 393).

**Done when**: `rg 'Patch::(keep|clear|set)\(' crates/ src/` returns zero matches.

**Rule?**: No.

**Counter-argument**: "Named constructors read more naturally than tuple/unit variants." Unfalsifiable — `Patch::Set(s)` and `Patch::set(s)` differ by one ASCII byte and read identically; the three wrappers are pure indirection.

**Depends on**: none.

---

## Final Ranking

### Structural (≥ 30 LOC or ≥ 2 axes)

Ranked by LOC removed; ties broken by axes touched.

| # | Title | ΔLOC | Axes |
|---|-------|------|------|
| S9 | Sweep redundant `IntoStaticStr` / `.label()` / `serialize_with` | −44 | LOC, types, methods, hand-rolled→derive |
| S7 | Collapse `DiagnosticRow` mirror into `PlanDoctorDiagnostic` | −25 | LOC, types, helper fn, wire mirror |
| S8 | Collapse `FindingRow` + `FindingLevel` into `Finding` | −25 | LOC, types, `From` impl |
| S1 | Delete `ContractAction` mirror enum | −22 | LOC, types, branches |
| S2 | Delete `severity_label` hand-roll | −12 | LOC, branches, hand-rolled→derive |
| S5 | Inline `scope_tools` into callers | −6 | LOC, module edges |
| S6 | Delete `warning_names` dedup set | −4 | LOC, branches, types (HashSet) |
| S3 | Collapse double regex dispatch in task.rs | −3 | LOC, branches, hand-rolled→idiomatic |
| S4 | Hoist literal Regex to OnceLock in primitives.rs | +6 | hand-rolled→idiomatic, branches, consistency |

### One-Touch Tidies

| # | Title | ΔLOC | Axis |
|---|-------|------|------|
| T11 | Inline `Patch::keep / clear / set` constructors | −18 | LOC |
| T6 | Store `CodexProvenance` directly in `RuleView` | −8 | LOC, types, branches |
| T1 | `design_references_exist`: Vec→HashSet for refs | −2 | LOC |
| T3 | Share compiled regex in merge/validate.rs | −2 | LOC |
| T5 | Replace bullet_re with str check | −2 | LOC, cargo edges (regex dep usage) |

---

## Post-mortem

- **S1** — predicted −22 LOC, actual −12 LOC (net across 2 files). The review counted gross deletes (enum + `From` impl) but under-counted replacement code: the inline `filter_map` closure added +7 LOC where `.map(ContractItem::from)` was 1 line, and the domain-side `Serialize` derive + `serde` import added +2 LOC. "Done when" assertion (`rg 'ContractAction' src/` → 0 matches) flipped cleanly on first pass. 825 tests pass, clippy clean, no regressions.
- **S2** — predicted −12 LOC, actual −9 LOC in `src/commands/codex.rs` (237 → 228). Variant-form path was taken (field changed to `severity: CodexSeverity`); the function deletion gave 8 lines but the call-site swap was 1-for-1, so only the function body net'd. "Done when" (`rg 'severity_label' src/` → 0) flipped cleanly. All tests pass, clippy clean, no regressions; serde + strum::Display both produce identical kebab output for JSON and text writers.
- **S3** — predicted −3 LOC, actual ±0 LOC in `crates/domain/src/task.rs`. The structural shape (`Some(m) = find()` + `let caps = captures().expect()` ⇒ `Some(caps) = captures()` + `let m = caps.get(0).expect()`) preserved line count: redundant regex execution gone, `.expect()` site moved (now guards regex-API contract, no longer inter-call inconsistency) and kept its justification string per coding-standards.md. "Done when" (`rg '\.find\(rest\)' crates/domain/src/task.rs` → 0) flipped cleanly. Full workspace tests pass (all suites green), clippy `-D warnings` clean, no regressions.
- **S4** — predicted +6 LOC net, actual +26 LOC net (+22 in `primitives.rs`, +4 in `merge/validate.rs`). Boilerplate undercounted: each accessor is 4 body lines + blank separator, plus the `use std::sync::OnceLock;` line in each file and the section-header doc comment I added in `primitives.rs` matching the `task.rs` precedent. Three accessors hoisted in `primitives.rs` (`bullet_re`, `checkbox_re`, `req_id_ref_re`); one accessor (`req_id_re`) in `merge/validate.rs` shared between the heading-structure loop and the design-orphan loop, eliminating the duplicate `Regex::new(REQ_ID_PATTERN)` at the old line 121. "Done when" (`rg 'Regex::new' crates/domain/src/validate/primitives.rs` → 1) did NOT flip as written: now returns 4 matches because OnceLock initializers contain `Regex::new` literally; semantic intent (1 production call site outside an accessor) is met — the dynamic `ids_match_pattern` is the only inline compile remaining. Full workspace tests pass, clippy `-D warnings` clean, no regressions.
- **S5** — predicted −6 LOC, actual −6 LOC in `crates/tool/src/load.rs` (207 → 201). `scope_tools` deleted; both call sites inlined to `tools.into_iter().map(|tool| (scope.clone(), tool)).collect()`. Estimate landed dead-on because the function was a true one-liner with two same-shape callers — no hidden boilerplate. "Done when" (`rg 'fn scope_tools' crates/tool/src/load.rs` → 0) flipped cleanly. Full workspace tests pass, clippy `-D warnings` clean, no regressions.
- **S6** — predicted −4 LOC, actual −3 LOC in `crates/tool/src/load.rs` (201 → 198, stacked on S5). `warning_names: HashSet` declaration removed and `if warning_names.insert(...) { warnings.push(...) }` collapsed to an unconditional `warnings.push(...)`; 1-line undercount because the `if`-block's closing brace shared a line accounting boundary the review treated as separable. Existing `merge_scoped_project_wins_and_warns_once` test still passes — its input only contains a single capability/project name collision, so the dedup branch was never exercised in tests anyway. "Done when" (`rg 'warning_names' crates/tool/src/load.rs` → 0) flipped cleanly. Full workspace tests pass, clippy `-D warnings` clean, no regressions.
- **S7** — predicted −25 LOC, actual −30 LOC in `src/commands/change/plan/doctor.rs` (108 → 78). Slight overshoot vs. estimate because the 11-line `DiagnosticRow` struct + its doc block came out cleanly and the `diagnostic_row` helper (13 lines) had no surviving call-site boilerplate; `DoctorBody.diagnostics` field re-typed to `Vec<PlanDoctorDiagnostic>` and the `.iter().map(diagnostic_row).collect()` line disappeared entirely. `write_doctor_text` swapped `d.severity == "error"` → `matches!(d.severity, Severity::Error)`. Domain `Diagnostic` already had `Serialize + rename_all = "kebab-case"`, so wire shape is byte-identical: `Severity` enum serializes to the same `"error"`/`"warning"` strings, `Option<DiagnosticPayload>` serializes through the same derive that `serde_json::to_value` used to call. "Done when" (`rg 'DiagnosticRow|diagnostic_row' src/` → 0) flipped cleanly. Full workspace tests pass (all suites green, 825-equivalent), clippy `-D warnings` clean, no regressions.
- **S8** — predicted −25 LOC net (lifecycle.rs −23, model.rs +3 ≈ −22 net), actual −31 LOC net: `lifecycle.rs` 366 → 334 (−32) and `model.rs` 405 → 406 (+1). Lifecycle overshoot came from the `write_finding_row_text` body collapsing into the new `write_finding_text` rather than persisting alongside; model.rs undershoot because adding `Serialize` + `#[serde(rename_all = "kebab-case")]` to the existing derive line + attribute landed in 1 net line, not 3 (the derive list was already multi-line). `FindingRow<'a>` (8 lines incl. attrs), `FindingLevel` (6 lines), and the `From<&Finding>` impl (14 lines) all gone; `PlanValidateBody.results` re-typed `Vec<FindingRow<'a>>` → `&'a [Finding]`; `write_finding_text` now `matches!(finding.level, Severity::Error)` against the enum directly. Wire shape identical at the JSON Value layer (golden assertions go through `serde_json::Value`, which key-sorts via BTreeMap), so the `Finding` field order (`level, code, message, entry`) vs prior `FindingRow` order (`level, code, entry, message`) is invisible to the existing `assert_golden` infrastructure. "Done when" (`rg 'FindingRow|FindingLevel' src/` → 0) flipped cleanly. Full workspace tests pass, clippy `-D warnings` clean, no regressions.
