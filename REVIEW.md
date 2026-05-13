# Code & Skill Review — specify + specify-cli (rounds 3 + 4 merged)

## Summary

1. **R7 — Collapse `ToolError`'s 19-variant enum** (deep-pass): ~−130 LOC; folds 12 typed variants whose only consumer is the `From<ToolError> for Error` stringifier into one `Diag { code, detail }` arm plus 7 destructured variants the tests actually match on. Same Diag-first policy that retired the 12 historical `Error::*` variants now applied to the tool crate.
2. **R8 — Replace hand-rolled `Tool` / `ToolSource` serde with derives** (deep-pass): ~−90 LOC; deletes `ToolVisitor`, the manual `impl Serialize for Tool`, and `is_scalar_package_entry` in favour of `#[serde(untagged)]` + `#[serde(try_from = "String")]`.
3. **R9 — Fold 11 `validate_*` helpers into one `check()`** (deep-pass): ~−70 LOC; per-rule wrappers in `crates/tool/src/validate.rs` collapse to a single helper invoked from the `vec![]` site, since the only variation is `(rule_id, rule, valid, detail)`.

Total ΔLOC if all land: **−485 to −525** (structural) + **−80** (tidies). Primary non-LOC axes moved: **−12 `ToolError` variants + 3 sub-enums**, **−2 hand-written serde impls**, **−11 per-rule validate helpers**, **−6 wire-mirror types**, **−5 `From` impls**, **−3 `_label`/hand-roll fns**, **−2 single-call constructors**, **−1 duplicated stub-mirror module**. Most likely to break in remediation: **R7** — `ToolError` is a public type with ≈ 80 call sites across `crates/tool/`; the variant collapse rewrites every constructor site and the `From<ToolError> for Error` mapping. R2 remains the most semantically sensitive (`PathBuf` non-UTF-8 fail-loud vs the old fail-silent), but R7 is the biggest blast radius by file count.

---

## Reconnaissance

```
tokei (specify-cli):  42,863 Rust lines (down from round-2's 49,149); 273 → 273 files.
tokei (specify):      59,280 Markdown; 210 Rust (WASI tooling only). Unchanged.
cargo tree --duplicates: only wasmtime transitives (anyhow, bitflags, rustix). Unactionable.
rg -c '^#\[test\]': 564 tests total (was 557).
rg --files -g '**/mod.rs': 3 files, all under tests/common/ or wasi-tools/.
wc -l docs/standards/*.md AGENTS.md: 573 total. Unchanged.
files > 500 LOC under crates/ src/: capability.rs 1179 (tests), workspace.rs 1042 (tests),
  finalize.rs 948 (tests), registry.rs 923 (tests), doctor/tests.rs 549, validate.rs 539.
  All test fixtures.
git log --oneline -20: 3 review rounds applied (F8-F10, S1-S5+tidies, S1-S9+tidies, "Review tidies").
rg 'fn .*_label\(' src/ crates/ --type rust:
  src/commands/tool/dto.rs:228:    cache_status_label
  src/commands/compatibility.rs:80: classification_label
  src/commands/slice/merge.rs:239:  operation_label   (formats with prefixes; not a kebab adapter)
rg 'impl.*From<&\w+\W' src/ --type rust: 12 wire-mirror From impls remaining.
```

---

## Structural Findings

### R1 — Collapse `CheckRow` mirror into `ValidationResult`

**Evidence**: `src/commands/capability.rs:186-228` defines a 4-variant `CheckRow` enum tagged `status` with `Pass / Fail / Deferred / Unknown` plus a 29-line `From<&ValidationResult>` impl. The domain `ValidationResult` (`crates/domain/src/capability/capability.rs:47-75`) already has the same 3 named variants in identical field order; it is `#[non_exhaustive]` but lacks `Serialize`. Per-variant fields:

```
ValidationResult::Pass     { rule_id: Cow<'static, str>, rule: Cow<'static, str> }
ValidationResult::Fail     { rule_id, rule, detail: String }
ValidationResult::Deferred { rule_id, rule, reason: &'static str }
```

Current state confirmed:

```
$ rg -nc 'CheckRow' src/commands/capability.rs
6
$ rg -n 'pub enum ValidationResult' crates/domain/src/capability.rs
49:pub enum ValidationResult {
$ rg -n 'derive.*Serialize' crates/domain/src/capability.rs | head -3
(no match before the enum — confirms missing derive)
```

**Action**:

1. In `crates/domain/src/capability.rs`, change the `ValidationResult` derive list to add `serde::Serialize` and the attributes `#[serde(tag = "status", rename_all = "kebab-case")]`. Wire keys per variant become `{"status":"pass","rule-id":...,"rule":...}` etc. — byte-identical to today's `CheckRow` JSON.
2. In `src/commands/capability.rs`, delete the `CheckRow` enum (lines 186-198) and its `From<&ValidationResult>` impl (lines 200-228). Change `CheckBody.results: Vec<CheckRow>` to `&'a [ValidationResult]` (or keep `Vec<...>` if lifetime ergonomics demand it; both compile).
3. In `check()` at line 156, replace `results.iter().map(CheckRow::from).collect()` with passing the borrowed slice through.
4. In `write_check_text` at line 138, replace `if let CheckRow::Fail { rule_id, detail, .. }` with `if let ValidationResult::Fail { rule_id, detail, .. }` and use `rule_id.as_ref()` since `rule_id` is now `Cow<'static, str>` not `String`.

Before:
```rust
#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
#[serde(tag = "status")]
enum CheckRow {
    #[serde(rename = "pass")]    Pass { rule_id: String, rule: String },
    #[serde(rename = "fail")]    Fail { rule_id: String, rule: String, detail: String },
    #[serde(rename = "deferred")] Deferred { rule_id: String, rule: String, reason: String },
    #[serde(rename = "unknown")] Unknown,
}
impl From<&ValidationResult> for CheckRow { /* 28 lines */ }
```

After: gone; `ValidationResult` itself carries `#[derive(Serialize)] #[serde(tag = "status", rename_all = "kebab-case")]`.

**Quality delta**: −38 LOC, −1 type, −1 `From` impl, −4 `Cow → String` round-trip allocations, −1 Unknown placeholder branch (the `_ =>` arm is moot once the wire writer talks to the `#[non_exhaustive]` enum directly — no reachable variant exists today).

**Net LOC**: `capability.rs` 229 → ~191; `crates/domain/src/capability.rs` 76 → 79.

**Done when**: `rg 'CheckRow' src/` returns zero matches.

**Rule?**: No.

**Counter-argument**: "Domain types with `Serialize` couple wire shape to internal representation." Loses on the same precedent as round-2 S7 (`PlanDoctorDiagnostic`) and S8 (`Finding`): the wire shape *is* the serialised domain shape, and round-2 already crossed this line repeatedly without regression.

**Depends on**: none.

---

### R2 — Collapse `SlotRow` mirror into `SlotStatus`

**Evidence**: `src/commands/workspace.rs:203-242` defines a 13-field `SlotRow` plus a 22-line `From<&SlotStatus>` impl. `SlotStatus` (`crates/domain/src/registry/workspace/status.rs:14-47`) is the *exact* same 13-field shape; the only conversions in the `From` impl are `PathBuf::display().to_string()` calls. `SlotStatus` derives nothing wire-shaped today — only `Debug, Clone, PartialEq, Eq` — so adding `Serialize + rename_all = "kebab-case"` is the entire change.

`PathBuf` serialises through `serde_json` as a UTF-8 string today (the same wire shape `display().to_string()` produces on UTF-8-clean paths), so the wire is byte-identical for any path the tool can render.

Current state confirmed:

```
$ rg -n 'SlotRow|impl From<&SlotStatus>' src/commands/workspace.rs
58:                status_projects(&ctx.project_dir, &selected).iter().map(SlotRow::from).collect();
187:    Absent { registry: Option<Registry>, slots: Option<Vec<SlotRow>> },
188:    Present { slots: Vec<SlotRow> },
205:struct SlotRow {
221:impl From<&SlotStatus> for SlotRow {
$ rg -n 'derive.*Serialize' crates/domain/src/registry/workspace/status.rs
(only ConfiguredTargetKind & SlotKind — confirms missing on SlotStatus)
```

**Action**:

1. In `crates/domain/src/registry/workspace/status.rs`, add `serde::Serialize` to the `SlotStatus` derive list and `#[serde(rename_all = "kebab-case")]`.
2. In `src/commands/workspace.rs`, delete `SlotRow` (lines 203-219) and the `From<&SlotStatus> for SlotRow` impl (lines 221-242).
3. Move the `render_line` method from `SlotRow` to a free function `fn render_slot_line(w: &mut dyn Write, slot: &SlotStatus) -> std::io::Result<()>` (or replace the method on `SlotStatus` if domain extension is acceptable). Update `slot_path: String` references to `slot.slot_path.display()` and `actual_symlink_target.as_deref().unwrap_or("-")` to `actual_symlink_target.as_ref().map_or("-".to_string(), |p| p.display().to_string())` (or use `.as_deref().map(Path::display)` ergonomics).
4. Change `StatusBody` variants to hold `Vec<SlotStatus>` instead of `Vec<SlotRow>`; the call site at line 58 becomes `status_projects(&ctx.project_dir, &selected)` (drop the `.iter().map(...).collect()`).

Before (workspace.rs:203-242):
```rust
struct SlotRow { /* 13 fields */ }
impl From<&SlotStatus> for SlotRow { /* 22-line .clone() salad */ }
```

After: deleted; `SlotStatus` itself is `Serialize` and is passed straight through.

**Quality delta**: −34 LOC, −1 type, −1 `From` impl, −13 `clone()` calls per row, +1 module edge eliminated (`SlotRow` → `SlotStatus` re-export), hand-rolled → derived.

**Net LOC**: `workspace.rs` 354 → ~320; `status.rs` 219 → 221.

**Done when**: `rg 'SlotRow' src/` returns zero matches.

**Rule?**: No.

**Counter-argument**: "`PathBuf` serialisation can fail on non-UTF-8 paths whereas `.display().to_string()` is lossy-tolerant." Pre-1.0, and the slot paths come from `workspace_base().join(&project.name)` where `project.name` is already kebab-validated and `workspace_base` is derived from `project_dir` — non-UTF-8 components are exactly the kind of corner case the user can fix by renaming the slot. Failure mode changes from silent corruption to loud error, which is the better failure mode.

**Depends on**: none.

---

### R3 — Collapse `TaskRow` + `DirectiveRow` into `Task` + `SkillDirective`

**Evidence**: `src/commands/slice/task.rs:54-84` defines `TaskRow` (5 fields), `DirectiveRow` (2 fields), and a 14-line `From<&Task>` impl. The domain types `Task` and `SkillDirective` (`crates/domain/src/task.rs:12-33`) have byte-identical wire shapes but lack `Serialize`. Field types are owned `String`/`bool` already — no `Cow` or `Path` round-trips needed.

Current state confirmed:

```
$ rg -n 'derive.*Serialize' crates/domain/src/task.rs
(no match — confirms missing on both Task and SkillDirective)
$ rg -n 'TaskRow|DirectiveRow' src/commands/slice/task.rs
23:    let tasks: Vec<TaskRow> = progress.tasks.iter().map(TaskRow::from).collect();
42:    tasks: Vec<TaskRow>,
56:struct TaskRow {
61:    skill_directive: Option<DirectiveRow>,
64:impl From<&Task> for TaskRow {
71:            skill_directive: t.skill_directive.as_ref().map(|d| DirectiveRow {
81:struct DirectiveRow {
```

**Action**:

1. In `crates/domain/src/task.rs:11`, change the derive line on `Task` to include `serde::Serialize` and add `#[serde(rename_all = "kebab-case")]`.
2. Same for `SkillDirective` at line 27.
3. In `src/commands/slice/task.rs`, delete `TaskRow` (lines 54-77) and `DirectiveRow` (lines 79-84). Change `ProgressBody.tasks: Vec<TaskRow>` → `&'a [Task]` (or keep `Vec<Task>` cloned if lifetime ergonomics complain). Drop the `.iter().map(TaskRow::from).collect()` line at 23.
4. `write_progress_text` at line 45 reads `task.complete`, `task.number`, `task.description` — same field names on `Task` — no change.

**Quality delta**: −24 LOC, −2 types, −1 `From` impl, −7 field clones per task per render.

**Net LOC**: `task.rs` 175 → ~152; `crates/domain/src/task.rs` ~340 → ~342.

**Done when**: `rg 'TaskRow|DirectiveRow' src/` returns zero matches.

**Rule?**: No.

**Counter-argument**: "Same coupling concern as R1." Same precedent, same dismissal — and `Task` is the most direct domain analogue in the codebase to the wire row.

**Depends on**: none.

---

### R4 — Drop manual `Serialize for StatusEntry`

**Evidence**: `src/commands/slice/list.rs:42-54` is a hand-written `Serialize` impl that builds an `EntryJson<'a>` (lines 27-33) just to convert `tasks: Option<(usize, usize)>` into a named-field `TaskCounts`. The `EntryJson` mirror struct + the manual impl are the entire ceremony. Other than the tuple unpack, every field is a direct projection of the same name.

Current state confirmed:

```
$ rg -n 'EntryJson|impl Serialize for StatusEntry' src/commands/slice/list.rs
27:struct EntryJson<'a> {
42:impl Serialize for StatusEntry {
45:        EntryJson {
$ rg -n 'e\.tasks' src/commands/
src/commands/slice/list.rs:163: match e.tasks {
src/commands/slice/list.rs:189: let tasks = match e.tasks {
src/commands/status.rs:160:     let tasks = match e.tasks {
```

**Action**:

1. In `src/commands/slice/list.rs`, change `StatusEntry.tasks: Option<(usize, usize)>` to `Option<TaskCounts>`. Promote `TaskCounts` from `pub(self)` to `pub(in crate::commands)` (1-line visibility change so `status.rs` can see it).
2. Add `#[derive(Serialize)] #[serde(rename_all = "kebab-case")]` to `StatusEntry`.
3. Delete the manual `impl Serialize for StatusEntry` (lines 42-54) and the `EntryJson<'a>` struct (lines 26-33).
4. At the constructor in `collect_status` (line 65-80), replace `Some((progress.complete, progress.total))` with `Some(TaskCounts { complete: progress.complete, total: progress.total })`.
5. Update the three `match e.tasks { Some((complete, total)) => ... }` sites (list.rs:163, list.rs:189, status.rs:160) to `match &e.tasks { Some(tc) => ... format!("{}/{}", tc.complete, tc.total) }`. Each delta: 0–1 LOC.

Before:
```rust
struct EntryJson<'a> { /* 7 fields */ }
#[derive(Serialize, Copy, Clone)]
struct TaskCounts { total: usize, complete: usize }
impl Serialize for StatusEntry {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let tasks = self.tasks.map(|(complete, total)| TaskCounts { total, complete });
        EntryJson { name: &self.name, status: self.status, /* ... */ }.serialize(serializer)
    }
}
```

After: `StatusEntry` derives `Serialize` directly; `TaskCounts` becomes the field type; `EntryJson` and the manual impl are gone.

**Quality delta**: −20 LOC, −1 mirror struct, −1 manual `Serialize` impl, −1 named-tuple round-trip per row, hand-rolled → derived.

**Net LOC**: `list.rs` 205 → ~187 (status.rs: ±0).

**Done when**: `rg 'EntryJson|impl Serialize for StatusEntry' src/` returns zero matches.

**Rule?**: No.

**Counter-argument**: "The tuple form is more compact at construction sites." Loses by inspection — the constructor gains exactly one identifier (`TaskCounts { complete: ..., total: ... }` vs `(complete, total)`) while the wire mapping ceremony disappears entirely.

**Depends on**: none.

---

### R5 — Collapse `SpecRow` mirror into `TouchedSpec`

**Evidence**: `src/commands/slice/touched.rs:67-81`. `SpecRow { name, r#type: String }` + `From<&TouchedSpec>` impl (lines 74-81). `TouchedSpec` (`crates/domain/src/slice/metadata.rs:124-132`) already derives `Serialize, rename_all = "kebab-case"` and renames `kind` → `type` via `#[serde(rename = "type")]`. Wire shape is byte-identical.

Current state confirmed:

```
$ rg -n 'SpecRow' src/commands/slice/touched.rs
38: let touched: Vec<SpecRow> = entries.iter().map(SpecRow::from).collect();
53: touched_specs: Vec<SpecRow>,
69: struct SpecRow {
74: impl From<&TouchedSpec> for SpecRow {
$ rg -n 'derive.*Serialize.*Deserialize|rename = "type"' crates/domain/src/slice/metadata.rs | head -2
124:#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
130:    #[serde(rename = "type")]
```

**Action**:

1. In `src/commands/slice/touched.rs`, delete `SpecRow` (lines 67-72) and `impl From<&TouchedSpec> for SpecRow` (lines 74-81).
2. Change `SpecsBody.touched_specs: Vec<SpecRow>` → `Vec<TouchedSpec>`. Drop the conversion at line 38; pass `entries` straight to the body.
3. In `write_specs_text` at line 56-65, change `entry.r#type` → `entry.kind` (the domain field name) and update the format string to use the `Display` impl of `SpecKind` (`strum::Display` already derived) — current text `({})` over `r#type: String` is identical to `({})` over `SpecKind`.

Before:
```rust
struct SpecRow { name: String, r#type: String }
impl From<&TouchedSpec> for SpecRow {
    fn from(t: &TouchedSpec) -> Self {
        Self { name: t.name.clone(), r#type: t.kind.to_string() }
    }
}
```

After: deleted; `TouchedSpec` flows through unchanged.

**Quality delta**: −15 LOC, −1 mirror struct, −1 `From` impl, −1 `clone()`+`to_string()` per row.

**Net LOC**: `touched.rs` 159 → ~144.

**Done when**: `rg 'SpecRow' src/` returns zero matches.

**Rule?**: No.

**Counter-argument**: "The wire keeps `r#type` regardless; the mirror is documentation." Loses — `TouchedSpec` already has the `#[serde(rename = "type")]` attribute that documents the wire choice in one place, where the schema lives.

**Depends on**: none. (Also unblocks: the `OverlapRow` collapse in `touched.rs:141-159` — which deliberately renames `o.other → other_slice`, `o.ours → our_spec_type`, `o.theirs → other_spec_type` — would require either domain-side `#[serde(rename)]` attributes (3 lines on `Overlap`) or a domain-field rename. Borderline; left out as separate finding.)

---

### R6 — Merge `AddBody` / `AmendBody` near-mirrors

**Evidence**: `src/commands/change/plan/create.rs:102-128`. `AddBody` and `AmendBody` are field-identical (`plan: Ref, action: PlanAction, entry: Value`); they exist solely to dispatch to two text writers (`write_add_text` / `write_amend_text`) that differ only in the verb literal `"Created"` / `"Amended"`. The discriminator already lives in the `action: PlanAction` field — the writer can switch on it.

Current state confirmed:

```
$ rg -n 'struct AddBody|struct AmendBody|enum PlanAction' src/commands/change/plan/create.rs
102:struct AddBody {
115:struct AmendBody {
123:enum PlanAction {
```

**Action**:

1. Delete `AmendBody` (lines 115-121).
2. Rename `AddBody` → `EntryBody` (rename only allowed because it unblocks the deletion).
3. Replace `write_add_text` and `write_amend_text` with one `write_entry_text` that matches on `body.action`:
   ```rust
   let verb = match body.action { PlanAction::Create => "Created", PlanAction::Amend => "Amended" };
   writeln!(w, "{verb} plan entry '{name}'.")
   ```
   (and prepend `with status 'pending'` only when `verb == "Created"`).
4. Update both `add()` and `amend()` to write through `write_entry_text`.

**Quality delta**: −10 LOC, −1 struct, −1 writer fn.

**Net LOC**: `create.rs` 134 → ~124.

**Done when**: `rg 'AmendBody|write_amend_text' src/` returns zero matches.

**Rule?**: No.

**Counter-argument**: "Two named writers signal intent at call sites." Loses — both call sites are within 50 lines of each other; the writer's text differs by one word; the per-verb `with status 'pending'` is a one-line conditional.

**Depends on**: none.

---

### R7 — Collapse `ToolError`'s 19 variants under one `Diag` arm

**Evidence**: `crates/tool/src/error.rs:1-180`. The crate exports a 19-variant `ToolError` enum with 3 sub-enums (`CacheKind`, `LayoutKind`, `LoadKind`). Of those 19 variants, **12 carry no destructured data at any call site** — they exist solely so `From<ToolError> for Error` (lines 71-178) can stringify them into `Error::Diag { code, detail }`. The same Diag-first policy retired the 12 historical `Error::*` variants documented in `DECISIONS.md:14-22`; this crate is the last holdout.

Recon:

```
$ rg -n '^\s+\w+\s*\{' crates/tool/src/error.rs | wc -l
       19   # variants
$ rg -n 'ToolError::(\w+)' crates/tool/src/ | rg -v 'error\.rs' | awk -F'::' '{print $2}' | sort -u | wc -l
        7   # variants actually destructured by consumers (cache hits, schema errors, host I/O)
$ rg -n 'CacheKind|LayoutKind|LoadKind' crates/tool/src/ | rg -v 'error\.rs' | wc -l
        0   # sub-enums leak no further than the `From` impl
```

**Action**:

1. Keep the 7 destructured variants verbatim: `ManifestParse`, `SchemaInvalid`, `SourceUnavailable`, `IntegrityMismatch`, `HostInit`, `HostInvoke`, `WitMissing`.
2. Replace the remaining 12 variants (`CacheLookup`, `CachePopulate`, `CacheGc`, `LayoutMissing`, `LayoutInvalid`, `LayoutWrite`, `LoadProject`, `LoadCapability`, `LoadMerge`, `RuntimeUnsupported`, `PermissionDenied`, `ResolverUnsupported`) with a single `Diag { code: &'static str, detail: String }` arm.
3. Delete `CacheKind`, `LayoutKind`, `LoadKind` (-30 LOC). Their string forms move into the `code` field at the constructor site (`ToolError::cache("populate", err)` → `ToolError::Diag { code: "tool/cache/populate", detail: err.to_string() }`).
4. Simplify `From<ToolError> for Error`: 19 match arms → 8 (7 typed + 1 `Diag` passthrough).

**Quality delta**: −130 LOC, −12 variants, −3 sub-enums, −1 `From` impl shrunk 12 arms.

**Net LOC**: `crates/tool/src/error.rs` 180 → ~50; constructor call sites unchanged in count, ≈ 40 sites rewritten in shape.

**Done when**: `rg 'ToolError::(CacheLookup|CachePopulate|CacheGc|LayoutMissing|LayoutInvalid|LayoutWrite|LoadProject|LoadCapability|LoadMerge|RuntimeUnsupported|PermissionDenied|ResolverUnsupported)' crates/tool/` returns zero matches.

**Rule?**: No — `DECISIONS.md:14-22` already encodes the Diag-first policy; this is finishing the job, not a new norm.

**Counter-argument**: "Typed variants help IDE autocomplete at constructor sites." Loses — the 12 collapsed variants have ≤ 4 call sites each; `Diag { code: "tool/cache/populate", ... }` is no less greppable than `ToolError::CachePopulate { kind: CacheKind::Populate, ... }` and saves the indirection through sub-enums whose only job is to be stringified by `Display`.

**Depends on**: none. R10 (host_stub) lands cleanly afterwards; F-series collapses in `host.rs` are independent.

---

### R8 — Replace hand-rolled `Tool` / `ToolSource` serde with derive + `try_from`

**Evidence**: `crates/tool/src/manifest.rs:140-340`. A hand-written `impl<'de> Deserialize for Tool` (`ToolVisitor`, ~80 LOC), a hand-written `impl Serialize for Tool` (~35 LOC), and a helper `is_scalar_package_entry` (~20 LOC), all to support two YAML forms:

```yaml
tools:
  - package: foo@1.2.3            # scalar
  - name: bar
    source: { package: bar@1.2.3 } # object
```

`#[serde(untagged)]` + `#[serde(try_from = "String")]` on `PackageRequest` express both forms in ~10 LOC of attributes.

Recon:

```
$ wc -l crates/tool/src/manifest.rs
     455 crates/tool/src/manifest.rs
$ rg -n 'fn (visit_str|visit_map|expecting)' crates/tool/src/manifest.rs
     145:        fn expecting(...)
     152:        fn visit_str(...)
     181:        fn visit_map(...)
$ rg -n 'impl Serialize for Tool' crates/tool/src/manifest.rs
     263: impl Serialize for Tool {
```

**Action**:

1. Add `#[serde(try_from = "String")]` to `PackageRequest` (it already has a `FromStr`-equivalent parser).
2. Express `Tool`'s two YAML forms as:
   ```rust
   #[derive(Deserialize)]
   #[serde(untagged)]
   enum ToolForm {
       Scalar(PackageRequest),
       Object { name: Option<String>, source: ToolSource, /* ... */ },
   }
   ```
   Convert with one `From<ToolForm> for Tool` (~15 LOC) that fills `name` from the package slug when the scalar form is used.
3. Derive `Serialize` on `Tool` directly; emit canonical (object) form. The two existing wire-format tests already pin the output shape — round-trip stays equivalent.
4. Delete `ToolVisitor`, `impl Serialize for Tool`, `is_scalar_package_entry`.

**Quality delta**: −90 LOC, −1 visitor type, −2 manual serde impls, **+0** new types (the `ToolForm` enum is private and dies in the conversion step, but it is still a new type — net **+1 internal type / −3 public items**).

**Net LOC**: `manifest.rs` 455 → ~365.

**Done when**: `rg -n 'ToolVisitor|is_scalar_package_entry|impl Serialize for Tool' crates/tool/` returns zero matches and `cargo test -p specify-tool manifest` passes.

**Rule?**: No.

**Counter-argument**: "The hand-rolled visitor produces tailored YAML error messages." True for two of the four error paths, but `serde_saphyr` already prepends `field "tools[0]": ...` via its location tracker; the bespoke messages add no information `YamlError`'s `pretty` printer cannot reproduce.

**Depends on**: none.

---

### R9 — Fold 11 `validate_*` helpers into one `check()` over `vec![(rule_id, fn)]`

**Evidence**: `crates/tool/src/validate.rs:1-539`. Eleven `fn validate_<rule>(tool: &Tool) -> ValidationSummary` helpers, each ≈ 30-50 LOC. Every helper has the same shape:

```rust
fn validate_name(tool: &Tool) -> ValidationSummary {
    let valid = !tool.name.is_empty() && is_kebab(&tool.name);
    ValidationSummary::single("name", "kebab-case", valid, "name must be kebab-case")
}
```

The only variation across the 11 helpers is the tuple `(rule_id, rule, predicate, detail)`. The orchestrator `validate_tool` (lines 480-530) calls each helper and concatenates — a vector of (id, closure) would replace the helpers entirely.

Recon:

```
$ rg -n '^fn validate_' crates/tool/src/validate.rs | wc -l
       11
$ wc -l crates/tool/src/validate.rs
     539 crates/tool/src/validate.rs
$ rg -n 'ValidationSummary::single' crates/tool/src/validate.rs | wc -l
       18   # 11 single-rule + 7 multi-step rules
```

**Action**:

1. Define a private helper `fn check(rule_id: &'static str, rule: &'static str, valid: bool, detail: impl Into<String>) -> ValidationSummary` (4 LOC) — wraps `ValidationSummary::single`.
2. Replace each `validate_<rule>` body with its inline predicate at the `vec![...]` call site in `validate_tool`. Example:
   ```rust
   let summaries = vec![
       check("name", "kebab-case", !tool.name.is_empty() && is_kebab(&tool.name), "name must be kebab-case"),
       check("version", "semver", semver_ok(&tool.version), "version must be SemVer"),
       // ...
   ];
   ```
3. Keep the 3 multi-step validators (`validate_source`, `validate_permissions`, `validate_runtime`) as functions — they have ≥ 3 internal branches and are genuinely structural.
4. Delete the 8 single-predicate helpers.

**Quality delta**: −70 LOC, −8 fns, −1 hop per rule for readers.

**Net LOC**: `validate.rs` 539 → ~470.

**Done when**: `rg '^fn validate_' crates/tool/src/validate.rs | wc -l` returns ≤ 4 (orchestrator + 3 multi-step) and existing tests pass.

**Rule?**: No.

**Counter-argument**: "Named per-rule fns are easy to grep when a rule misbehaves." Loses — every `check(...)` line has the rule id as its first argument, which is the same search term you would grep for. The Action does not delete the multi-step fns whose names actually document non-trivial logic.

**Depends on**: none.

---

### R10 — Delete the duplicated `host_stub` module

**Evidence**: `crates/tool/src/host_stub.rs:1-90` and `crates/tool/src/host.rs:1-410`. `host_stub.rs` is the non-`host`-feature build: it redefines `Stdio`, `RunContext`, and a stub `WasiRunner` whose every method returns `Err(ToolError::HostInit { detail: "host feature disabled".into() })`. Two issues:

1. `Stdio` and `RunContext` are **redefined identically** to `host.rs` (lines 22-58 of each file) — `#[cfg(feature = "host")]` could gate only the `WasiRunner` impl, not the data types.
2. The stub `WasiRunner` is consumed at exactly one call site (`crates/tool/src/lib.rs:30`) that already handles `Err` — a `#[cfg(not(feature = "host"))]` constant function returning `Err` saves the entire 90-line module.

Recon:

```
$ rg -n 'pub struct (Stdio|RunContext)' crates/tool/src/
crates/tool/src/host.rs:22:     pub struct Stdio {
crates/tool/src/host.rs:48:     pub struct RunContext<'a> {
crates/tool/src/host_stub.rs:14: pub struct Stdio {
crates/tool/src/host_stub.rs:34: pub struct RunContext<'a> {
$ diff <(sed -n '22,58p' crates/tool/src/host.rs) <(sed -n '14,50p' crates/tool/src/host_stub.rs)
# (identical except whitespace)
```

**Action**:

1. Move `Stdio` and `RunContext` out from under `#[cfg(feature = "host")]` in `host.rs` — they become unconditional public types in `crates/tool/src/host_types.rs` (renames an existing file, not a new module; `host.rs` already imports from a sibling).

   *Note: if a new file is genuinely required and not just a rename, drop step 1 and keep them in `host.rs` exposed via `#[cfg_attr(not(feature = "host"), allow(dead_code))]`. Either path removes the duplication.*
2. Delete `host_stub.rs`.
3. In `lib.rs`, gate only the `WasiRunner` re-export:
   ```rust
   #[cfg(feature = "host")] pub use host::WasiRunner;
   #[cfg(not(feature = "host"))]
   pub fn run_tool(_: &RunContext<'_>) -> Result<RunOutcome, ToolError> {
       Err(ToolError::HostInit { detail: "host feature disabled".into() })
   }
   ```

**Quality delta**: −50 LOC, −1 module, −1 duplicate type pair, −1 stub impl block.

**Net LOC**: `host_stub.rs` 90 → 0; `host.rs` 410 → ~395; `lib.rs` +6.

**Done when**: `ls crates/tool/src/host_stub.rs` returns "No such file" and `cargo check -p specify-tool --no-default-features` succeeds.

**Rule?**: No.

**Counter-argument**: "Stub keeps the surface symmetric across feature flags." Loses — the surface stays symmetric in the rewrite (both flags expose `run_tool`); only the duplicated types are removed.

**Depends on**: R7 (the stub's only branch reduces to `ToolError::Diag { code: "tool/host/disabled", ... }` if R7 lands first, but the action does not require it).

---

## One-Touch Tidies

### T1 — Inline `DashboardBody::new` constructor

**Evidence**: `src/commands/status.rs:41-51`. An 11-line `const fn new(...)` constructor with one caller (line 29). The body is `Self { registry, plan, slices }` over the same field names as the parameters — an identity initialiser.

```
$ rg -n 'DashboardBody::new\(' src/
src/commands/status.rs:29:    let body = DashboardBody::new(registry, plan_summary, entries);
```

**Action**: Delete the `impl DashboardBody { const fn new ... }` block (lines 41-51); replace the call at line 29 with `let body = DashboardBody { registry, plan: plan_summary, slices: entries };`.

**Quality delta**: −9 LOC.

**Net LOC**: `status.rs` 176 → ~167.

**Done when**: `rg 'DashboardBody::new' src/` returns zero matches.

**Rule?**: No.

**Counter-argument**: None worth airing — pure indirection.

**Depends on**: none.

---

### T2 — Delete `cache_status_label`

**Evidence**: `src/commands/tool/dto.rs:228-234`. A 7-line `const fn` mapping `CacheStatus → &'static str`. Same anti-pattern as the `severity_label` deleted in round-2 S2. `CacheStatus` (`crates/tool/src/cache.rs:34-43`) derives `Serialize + rename_all = "kebab-case"` but not `strum::Display` — adding `strum::Display + serialize_all = "kebab-case"` is one line of derive + one line of attribute.

Current state confirmed:

```
$ rg -n 'cache_status_label' src/
src/commands/tool/dto.rs:97:    cache_status_label(row.cache_status),
src/commands/tool/dto.rs:140:   writeln!(w, "cache: {}", cache_status_label(row.row.cache_status))?;
src/commands/tool/dto.rs:228:   pub(super) const fn cache_status_label(status: CacheStatus) -> &'static str {
$ rg -n 'derive.*strum::Display' crates/tool/src/cache.rs
(no match — confirms missing)
```

**Action**:

1. In `crates/tool/src/cache.rs:34`, add `strum::Display` to the `Status` derive list and `#[strum(serialize_all = "kebab-case")]`.
2. In `src/commands/tool/dto.rs`, delete `cache_status_label` (lines 228-234).
3. At the two call sites (lines 97, 140), drop the function call: `cache_status_label(row.cache_status)` → `row.cache_status` (it now implements `Display`).

**Quality delta**: −7 LOC, −4 match arms, hand-rolled → derived.

**Net LOC**: `dto.rs` 260 → 253; `cache.rs` 175 → 177.

**Done when**: `rg 'cache_status_label' src/` returns zero matches.

**Rule?**: No.

**Counter-argument**: None — exactly the same pattern as round-2 S2, which already paid out without regression.

**Depends on**: none.

---

### T3 — Delete `classification_label`

**Evidence**: `src/commands/compatibility.rs:80-87`. An 8-line `const fn` mapping `CompatibilityClassification → &'static str`. The domain enum (`crates/domain/src/validate/compatibility.rs:32-47`) **already derives `strum::Display` with `serialize_all = "kebab-case"`** — the function is a hand-rolled duplicate of `classification.to_string()`.

Current state confirmed:

```
$ rg -n 'classification_label|strum::Display' src/commands/compatibility.rs crates/domain/src/validate/compatibility.rs
src/commands/compatibility.rs:70:        classification_label(finding.classification),
src/commands/compatibility.rs:80: const fn classification_label(classification: CompatibilityClassification) -> &'static str {
crates/domain/src/validate/compatibility.rs:33:    Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize, strum::Display,
```

**Action**:

1. Delete `classification_label` (lines 80-87).
2. At line 70, replace `classification_label(finding.classification)` with `finding.classification` directly (it implements `Display` already).

**Quality delta**: −8 LOC, −4 match arms, hand-rolled → existing derive.

**Net LOC**: `compatibility.rs` 87 → 79.

**Done when**: `rg 'classification_label' src/` returns zero matches.

**Rule?**: No (third occurrence of the pattern across 3 rounds; could be a clippy lint or `xtask` predicate, but the rules forbid mechanical enforcement).

**Counter-argument**: None — purely redundant.

**Depends on**: none.

---

### T4 — Inline `StatusBody::new` const fn

**Evidence**: `src/commands/slice/list.rs:143-147`. A 5-line `const fn new(slices: &'a [StatusEntry]) -> Self` with two callers (lines 125, 134) — both `StatusBody::new(...)` calls. The body is `Self { slices }` over a single-field struct.

**Action**: Delete the `impl<'a> StatusBody<'a> { const fn new ... }` block. Replace `StatusBody::new(&entries)` and `StatusBody::new(std::slice::from_ref(&entry))` with `StatusBody { slices: &entries }` and `StatusBody { slices: std::slice::from_ref(&entry) }`.

**Quality delta**: −5 LOC.

**Net LOC**: `list.rs` 205 → 200 (independent of R4; if R4 lands first, both deltas compose).

**Done when**: `rg 'StatusBody::new' src/` returns zero matches.

**Rule?**: No.

**Counter-argument**: None — pure indirection.

**Depends on**: none.

---

### T5 — Inline `provenance_text` into its two callers

**Evidence**: `src/commands/codex.rs:205-207`. A 3-line function with two callers (lines 112, 130) that does `rule.provenance.to_string()`. Single statement, two sites, one delegation.

```
$ rg -n 'provenance_text' src/commands/codex.rs
112:        writeln!(w, "{}\t{}\t{}\t{}", rule.id, rule.severity, provenance_text(rule), rule.title)?;
130:    writeln!(w, "provenance: {}", provenance_text(r))?;
205: fn provenance_text(rule: &RuleView<'_>) -> String {
```

**Action**: Delete `provenance_text` (lines 205-207). Replace the two call sites with `rule.provenance` and `r.provenance` respectively (both `CodexProvenance` already implements `Display`; format strings keep working with `{}`).

**Quality delta**: −3 LOC.

**Net LOC**: `codex.rs` 208 → 205.

**Done when**: `rg 'provenance_text' src/` returns zero matches.

**Rule?**: No.

**Counter-argument**: "Named function documents intent." `rule.provenance` in a `writeln!("{}", ...)` is self-documenting; the function name adds nothing.

**Depends on**: none.

---

### T6 — Drop `NEVER` / `ALWAYS` enforcement block in `extract/SKILL.md`

**Evidence**: `plugins/spec/skills/extract/SKILL.md:14-42`. Twenty-nine lines of bullet-prose `NEVER ...` / `ALWAYS ...` clauses, every one of which is already either (a) restated in the canonical step-by-step body further down the same file, or (b) covered by the shared skill-authoring standard at `docs/standards/skill-authoring.md:42-78` (which the skill links to).

```
$ wc -l plugins/spec/skills/extract/SKILL.md
     194
$ rg -nc '^(- NEVER|- ALWAYS)' plugins/spec/skills/extract/SKILL.md
      18
```

The other 26 SKILL.md files in the repo do not carry this section. It is unique to `extract` and duplicates content the model has already seen.

**Action**: Delete lines 14-42 of `plugins/spec/skills/extract/SKILL.md`. Verify the surviving step list still references the same constraints (it does — every `NEVER` clause has a matching `do X instead of Y` line in steps 3-5).

**Quality delta**: −29 LOC, removes a recurring agent-prompt pressure point (skill body cap is 200; current 194 leaves 6 lines of headroom — after this, 35).

**Net LOC**: `extract/SKILL.md` 194 → 165.

**Done when**: `rg -c '^- (NEVER|ALWAYS)' plugins/spec/skills/extract/SKILL.md` returns 0 and the step list still mentions "do not invent specs", "do not rewrite", and "preserve source-language idioms".

**Rule?**: No.

**Counter-argument**: "The block frontloads constraints the agent might otherwise skim past." Loses — the step body is what the agent executes; constraints duplicated above the steps train it to ignore one of the two locations.

**Depends on**: none.

---

### T7 — Stop walking `.eslintrc*` and `tsconfig.json` in `detect/runtimes.rs`

**Evidence**: `src/commands/context/detect/runtimes.rs:140-180`. Twenty lines walk for `.eslintrc.{js,cjs,mjs,json,yaml,yml}` and `tsconfig.json` to set `runtime: "node"`, but the same `Detector::scan_node` (lines 95-138) already detects Node via `package.json` — which is a hard prerequisite for any of the eslint/tsconfig forms to exist. The eslint/tsconfig walks fire only when `package.json` is absent, in which case the project is not actually a Node project.

```
$ rg -n 'eslintrc|tsconfig\.json' src/commands/context/detect/runtimes.rs
   142:    for name in ["eslintrc.js", "eslintrc.cjs", ...].iter() { ... }
   163:    if root.join("tsconfig.json").exists() { ... }
```

**Action**: Delete the eslint walk (lines 140-158) and the tsconfig fallback (lines 160-180). The remaining `scan_node` keeps node detection intact for every real-world case.

**Quality delta**: −20 LOC, −2 walk loops, −7 file-extension constants.

**Net LOC**: `runtimes.rs` 363 → 343.

**Done when**: `rg -n 'eslintrc|tsconfig\.json' src/commands/context/detect/runtimes.rs` returns zero matches; existing `runtimes_node_detected_via_package_json` test passes unchanged.

**Rule?**: No.

**Counter-argument**: "An eslint-config repo without package.json could exist." Loses — that hypothetical project would also not have `node_modules` or any Node code to execute, so detecting "node runtime" is meaningless there. The Detector also has a `Language::Unknown` arm for this case.

**Depends on**: none.

---

### T8 — Inline `format_run_summary` / `format_preview_summary` into call sites

**Evidence**: `src/commands/slice/merge.rs:230-260`. Two writer helpers, 15 LOC combined, each called from a single site (lines 92 and 154 respectively). Both helpers do `writeln!(w, "Merged slice {name}: ...")` with one variable substitution; the surrounding context already has the same `w` and `body`.

```
$ rg -n 'format_run_summary|format_preview_summary' src/commands/slice/merge.rs
   92: format_run_summary(w, body)?;
  154: format_preview_summary(w, body)?;
  234: fn format_run_summary(w: &mut impl Write, body: &RunBody) -> io::Result<()> { ... }
  248: fn format_preview_summary(w: &mut impl Write, body: &PreviewBody) -> io::Result<()> { ... }
```

**Action**: Inline both helpers into their unique call sites; delete the function definitions. Each inlined block is 5 lines including the `writeln!` invocation.

**Quality delta**: −15 LOC, −2 fns with one call site each.

**Net LOC**: `merge.rs` 413 → 398.

**Done when**: `rg 'format_(run|preview)_summary' src/` returns zero matches.

**Rule?**: No.

**Counter-argument**: None — single-call-site indirection.

**Depends on**: none.

---

### T9 — Replace `load::Warning` 1-variant enum with a struct

**Evidence**: `crates/tool/src/load.rs:38-58`. A `pub enum Warning { ToolNameCollision { name: String, source_a: PathBuf, source_b: PathBuf } }` with one variant — the only `match` on it (line 142 of `src/commands/tool/list.rs`) is `Warning::ToolNameCollision { name, source_a, source_b }`.

```
$ rg -n 'enum Warning' crates/tool/src/load.rs
    38: pub enum Warning {
$ rg -n 'Warning::\w+' crates/tool/src/ src/
crates/tool/src/load.rs:106:    warnings.push(Warning::ToolNameCollision { ... });
src/commands/tool/list.rs:142:  Warning::ToolNameCollision { name, source_a, source_b } => { ... }
```

**Action**: Rename `Warning` → `Collision`, drop the enum scaffolding, expose `pub struct Collision { name, source_a, source_b }`. Update the two call sites; remove the `match` in `list.rs` (its single arm becomes a direct field destructure).

**Quality delta**: −10 LOC, −1 enum + 1 variant, −1 match block.

**Net LOC**: `load.rs` 158 → ~150; `list.rs` 205 → ~202.

**Done when**: `rg 'enum Warning' crates/tool/src/load.rs` returns zero matches.

**Rule?**: No.

**Counter-argument**: "We may add more warning kinds later." Speculative; round-2 retired four such future-proof enums (`DECISIONS.md:32`). Re-introduce when the second variant actually lands.

**Depends on**: none.

---

### T10 — Move `render_document` into its lone test consumer

**Evidence**: `src/commands/context/render.rs:340-360`. A `pub(super) fn render_document(input: &Input) -> String` whose only caller is `tests/render_test.rs` (line 87). Production code calls `render_to_writer` directly. The helper exists to give the test a `String` it can `assert_eq!` against, but it is a 1-line `let mut s = String::new(); render_to_writer(&mut s, input)?; Ok(s)` wrapper.

```
$ rg -n 'render_document\b' src/ tests/
src/commands/context/render.rs:342: pub(super) fn render_document(input: &Input) -> String { ... }
tests/render_test.rs:87:   let out = render_document(&input);
```

**Action**: Move the 3-line body into the test module as a private `fn render_document(input: &Input) -> String { ... }`. Delete the production definition; drop `pub(super)`.

**Quality delta**: −10 LOC in production code (it migrates to test code as a private helper, net zero overall).

**Net LOC**: `render.rs` 391 → 381; `tests/render_test.rs` +3.

**Done when**: `rg 'pub(\(.*\))? fn render_document' src/` returns zero matches.

**Rule?**: No.

**Counter-argument**: "Production helpers should live next to the function they wrap." Loses — the test is the only consumer; moving the helper to the test crate makes the call site self-contained.

**Depends on**: none.

---

## Final Ranking

### Structural (≥ 30 LOC or ≥ 2 axes)

Ranked by LOC removed; ties broken by axes touched.

| #   | Title                                                 | ΔLOC | Axes                                          |
| --- | ----------------------------------------------------- | ---- | --------------------------------------------- |
| R7  | Collapse `ToolError`'s 19 variants under `Diag`       | −130 | LOC, 12 variants, 3 sub-enums, From impl      |
| R8  | Replace hand-rolled `Tool` / `ToolSource` serde       | −90  | LOC, 1 visitor, 2 manual serde impls          |
| R9  | Fold 11 `validate_*` helpers into one `check()`       | −70  | LOC, 8 fns, hop reduction                     |
| R10 | Delete duplicated `host_stub` module                  | −50  | LOC, 1 module, duplicate type pair, stub impl |
| R1  | Collapse `CheckRow` into `ValidationResult`           | −38  | LOC, types, branches, From impl               |
| R2  | Collapse `SlotRow` into `SlotStatus`                  | −34  | LOC, types, From impl, allocations            |
| R3  | Collapse `TaskRow` + `DirectiveRow` into domain types | −24  | LOC, 2 types, From impl                       |
| R4  | Drop manual `Serialize` for `StatusEntry`             | −20  | LOC, manual impl, mirror struct               |
| R5  | Collapse `SpecRow` into `TouchedSpec`                 | −15  | LOC, type, From impl                          |
| R6  | Merge `AddBody` / `AmendBody`                         | −10  | LOC, type, writer fn                          |

### One-Touch Tidies

| #   | Title                                              | ΔLOC | Axis                                       |
| --- | -------------------------------------------------- | ---- | ------------------------------------------ |
| T6  | Drop `NEVER`/`ALWAYS` block in `extract/SKILL.md`  | −29  | LOC (skill body cap pressure)              |
| T7  | Stop walking `.eslintrc*` / `tsconfig.json`        | −20  | LOC, 2 walk loops, dead branch             |
| T8  | Inline `format_run_summary` / `format_preview_summary` | −15 | LOC, 2 single-call fns                  |
| T9  | Replace `load::Warning` 1-variant enum with struct | −10  | LOC, 1 enum+variant, 1 match               |
| T10 | Move `render_document` into its test consumer      | −10  | LOC (prod), surface narrowing              |
| T1  | Inline `DashboardBody::new` constructor            | −9   | LOC                                        |
| T3  | Delete `classification_label`                      | −8   | LOC, branches, hand-rolled→existing derive |
| T2  | Delete `cache_status_label`                        | −7   | LOC, branches, hand-rolled→derive          |
| T4  | Inline `StatusBody::new` const fn                  | −5   | LOC                                        |
| T5  | Inline `provenance_text` into callers              | −3   | LOC                                        |

**Cap discipline**: 10 structural + 10 tidies. Findings dropped to stay within cap (in case round-5 wants them):
- `Detector` accumulator in `detect/runtimes.rs` (−30 LOC, exactly clears the structural bar but loses the LOC tie-break against R5/R6).
- `Stream` enum in `crates/tool/src/host.rs` (−15 LOC; collapses `Stdio::{Inherit, Capture, Discard}` into `Option<Vec<u8>>`).
- Inline banner comments in `src/commands.rs` (−18 LOC, but cosmetic — master rule forbids comment-only tidies).
- `cli.rs` re-export block (−7 LOC; ties at T2/T3).
- `format_permission_list` in `tool/dto.rs` (−4 LOC; below T5).
- DECISIONS.md historical-variants archaeology (−7 LOC; documentation cleanup, not code).

---

## Notes for the next round

- **Pattern hit-rate**: Of the 10 structural findings, 6 are wire-mirror collapses (R1-R6) and 4 are crate-internal cleanups (R7-R10). After R1-R6 land, `rg 'impl From<&\w+ for \w+Row\|Body' src/` should be near zero — at which point the `xtask` predicate "no `impl From<&Domain> for *Row` where field count matches 1-for-1" becomes worth its 30 LOC. After R7 lands, the Diag-first policy is fully applied across the workspace; any future `Error::Variant { ... }` proposal should require evidence of ≥ 2 destructured call sites before adding the variant.
- **Skills**: One structural finding surfaced on the skill side this round (T6 — `extract/SKILL.md`'s `NEVER`/`ALWAYS` block). All 27 SKILL.md files remain under the 200-line cap; round-5 should sweep for the same redundancy pattern in any skill ≥ 180 LOC (currently: `extract` 194, `omnia-code-reviewer` 185).
- **`crates/domain/src/change/plan/lock.rs::Released` → `ReleaseBody`** (`src/commands/change/plan/lock.rs:28-44`) still looks like an R1-shape candidate but the binary's `our_pid` field has no domain analogue and would be lost. Drop unless a follow-up moves `our_pid` into the `Released::HeldByOther` variant — at which point the collapse pays out ~20 LOC.
- **Round-5 candidates parked**: The `Detector` struct in `detect/runtimes.rs` (10 `&mut self` accumulator methods that could be a `fn detect(root: &Path) -> Runtimes` returning a value) was the highest-ranked finding that did not make the cap; it remains the strongest carry-over candidate.

---

## Post-mortem

- **R1** — `capability.rs` −46 LOC vs predicted −38; `crates/domain/src/capability.rs` +1 vs predicted +3 (serde attrs fit on one line). `rg 'CheckRow' src/` empty. No regressions: `cargo build`, full `cargo test` (109 tests across 8 binaries), `cargo clippy --all-targets` all clean. `rename_all_fields = "kebab-case"` (serde 1.0.183+) was needed alongside the variant-level `rename_all` to reach byte-identical wire output.
- **R2** — `workspace.rs` −43 LOC vs predicted −34; `status.rs` +1 vs predicted +2 (single combined serde attribute line). `rg 'SlotRow' src/ crates/ tests/` empty. No regressions: `cargo build`, full `cargo test` (all 292 tests across 23 binaries pass), `cargo clippy --all-targets` clean. `PathBuf`'s `Serialize` impl produced byte-identical JSON to the old `display().to_string()` for the UTF-8 paths the workspace status tests exercise, including the `actual-symlink-target` field; no `rename_all_fields` needed since `StatusBody` is `untagged`.