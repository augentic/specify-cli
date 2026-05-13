# Code & Skill Review — single pass, quality-biased

**Top three by LOC removed**: (1) collapse triple `for project in &self.projects` walks in `registry/validate.rs` (≈ −25 LOC, −2 branches); (2) inline the `Stream` enum + `writer_for` indirection in `src/output.rs` (≈ −14 LOC, −1 type, −1 branch); (3) delete the one-function `serde_helpers` module and use `std::ops::Not::not` (−11 LOC, −1 module edge). **If all land**: ≈ −70 LOC across `crates/domain/src/registry/validate.rs`, `src/output.rs`, `crates/domain/src/{serde_helpers.rs,config.rs,lib.rs}`, `crates/tool/src/manifest.rs`, `src/commands/tool/dto.rs`, and `src/commands/slice/merge.rs`, plus −4 types/branches and −1 file/module edge. **Primary non-LOC axes moved**: types, branches, module edges. **Most likely to break in remediation**: S1 (collapsed `validate_shape` loop) — the four invariants currently fast-fail in a documented order; reordering can change which `Error::Diag.code` operators see when a registry breaks multiple rules at once.

---

## Structural findings

### S1. Collapse triple `for project` walks in `validate_shape`

- **Evidence**: `crates/domain/src/registry/validate.rs:97-152` runs `for project in &self.projects { if let Some(roles) = &project.contracts { ... } }` three times back-to-back for invariants 3 (path validity), 4 (self-consistency), 1 (single producer). The function carries `#[expect(clippy::too_many_lines, reason = "Single fast-fail validator: one block per shape rule keeps the policy table auditable.")]` at `:21-24` precisely because of this pattern. Current state:

```98:152:crates/domain/src/registry/validate.rs
        for project in &self.projects {
            if let Some(roles) = &project.contracts {
                for path in roles.produces.iter().chain(roles.consumes.iter()) {
                    if path.starts_with('/') || path.contains("..") {
// ... two more identical `for project / if let Some(roles)` blocks for invariants 4 and 1 follow ...
```

`rg -c 'for project in &self.projects' crates/domain/src/registry/validate.rs` → `4` (one shape pass, three contract passes).

- **Action**: fold invariants 1/3/4 into the existing shape-validation loop at `:37`. Make `producers: HashMap<&str, &str>` a mutable accumulator declared above the loop; do path validity + produces/consumes self-consistency + single-producer registration in the same per-project block as the existing name/url/capability/description checks. Delete the three trailing loops and the `#[expect(clippy::too_many_lines, ...)]` attribute.
- **Quality delta**: −≈25 LOC, −2 branches, −1 clippy `#[expect]` override.
- **Net LOC**: 280 → ≈255 in `validate.rs`.
- **Done when**: `rg -c 'for project in &self.projects' crates/domain/src/registry/validate.rs` → `1`, and `rg 'too_many_lines' crates/domain/src/registry/validate.rs` → no matches.
- **Rule?**: No.
- **Counter-argument**: "one block per rule is auditable" — loses because the four blocks share an identical loop header and `if let Some(roles)`; the auditability claim is undermined by the duplication itself. Documented diagnostic-code ordering must be preserved (see "most likely to break" above).
- **Depends on**: none.

### S2. Inline `Stream` enum + `writer_for` in `src/output.rs`

- **Evidence**: `src/output.rs:15-19` declares `enum Stream { Stdout, Stderr }`. `:124-129` is the only consumer:

```122:129:src/output.rs
/// Return a locked stdout/stderr writer for `stream`. Boxed to keep
/// the JSON and text emitter signatures uniform across both sinks.
fn writer_for(stream: Stream) -> Box<dyn Write> {
    match stream {
        Stream::Stdout => Box::new(std::io::stdout().lock()),
        Stream::Stderr => Box::new(std::io::stderr().lock()),
    }
}
```

`rg 'Stream::' src/ -t rust` shows the enum is only ever passed at two literal sites: `write()` always passes `Stream::Stdout` (`:34`), `report()` always passes `Stream::Stderr` (`:114`). No third caller.

- **Action**: drop `enum Stream` and `fn writer_for`. Change `emit` to take `mut writer: Box<dyn Write>` (or `&mut dyn Write`) as its first parameter. `write()` passes `Box::new(std::io::stdout().lock())`; `report()` passes `Box::new(std::io::stderr().lock())`. Update doc references to `Stream::Stderr` in the module-level docs to `std::io::stderr()`.
- **Quality delta**: −≈14 LOC, −1 type, −1 branch, −1 helper fn.
- **Net LOC**: 199 → ≈185.
- **Done when**: `rg -c 'enum Stream' src/output.rs` → `0`, and `rg -c 'fn writer_for' src/output.rs` → `0`.
- **Rule?**: No.
- **Counter-argument**: "the enum documents stdout-vs-stderr intent" — loses because the parameter type is the same `Box<dyn Write>` either way, and the two call sites already name `stdout()`/`stderr()` literally; ripgrep, jj, and cargo all dispatch this way without a `Stream` enum.
- **Depends on**: none.

### S3. Delete `serde_helpers` — replace `is_false` with `std::ops::Not::not`

- **Evidence**: `crates/domain/src/serde_helpers.rs` is 9 lines for one predicate. `rg 'is_false' src/ crates/ -t rust` returns exactly two matches: the definition and the single call site at `crates/domain/src/config.rs:57`:

```56:58:crates/domain/src/config.rs
    #[serde(default, skip_serializing_if = "crate::serde_helpers::is_false")]
    pub hub: bool,
```

- **Action**: delete `crates/domain/src/serde_helpers.rs`; remove `pub mod serde_helpers;` from `crates/domain/src/lib.rs:12`; change the `skip_serializing_if` value to `"std::ops::Not::not"`.
- **Quality delta**: −10 LOC across the crate, −1 file, −1 module edge in `lib.rs`, −1 cross-module `use`.
- **Net LOC**: `serde_helpers.rs` (9) + `lib.rs` (1) + `config.rs` (0) → 0.
- **Done when**: `rg --files crates/domain/src | rg serde_helpers` → no matches; `rg 'is_false' src/ crates/ -t rust` → no matches.
- **Rule?**: No.
- **Counter-argument**: "named predicate documents intent" — loses because the field is `hub: bool` and `skip_serializing_if = "std::ops::Not::not"` is the documented serde idiom (serde docs reference this exact form for boolean fields).
- **Depends on**: none.

### S4. Replace `ToolScopeKind` mirror with `strum::EnumDiscriminants` on `ToolScope`

- **Evidence**: `src/commands/tool/dto.rs:46-54` hand-rolls a serializable mirror of the `ToolScope` discriminant from `crates/tool/src/manifest.rs:225-239`:

```46:54:src/commands/tool/dto.rs
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, serde::Deserialize, strum::Display,
)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub(super) enum ToolScopeKind {
    Project,
    Capability,
}
```

The mirror exists because `ToolScope::Capability` carries a non-serialisable `PathBuf` (`crates/tool/src/manifest.rs:237`). The only producer is `scope_labels` at `src/commands/tool/dto.rs:223-230`, which already does the `ToolScope → ToolScopeKind` projection by hand:

```223:230:src/commands/tool/dto.rs
pub(super) fn scope_labels(scope: &ToolScope) -> (ToolScopeKind, String) {
    match scope {
        ToolScope::Project { project_name } => (ToolScopeKind::Project, project_name.clone()),
        ToolScope::Capability { capability_slug, .. } => {
            (ToolScopeKind::Capability, capability_slug.clone())
        }
    }
}
```

`strum` is already a dependency of `crates/tool` (`crates/tool/Cargo.toml:37`) with the `derive` feature pulled in via the workspace declaration (`Cargo.toml:190`). `EnumDiscriminants` is the documented strum mechanism for exactly this case.

- **Action**: derive `strum::EnumDiscriminants` on `ToolScope` in `crates/tool/src/manifest.rs:225`, naming the discriminant `ToolScopeKind` and forwarding the same derives the hand-rolled enum carries:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash, strum::EnumDiscriminants)]
#[strum_discriminants(
    name(ToolScopeKind),
    derive(Hash, serde::Serialize, serde::Deserialize, strum::Display),
    serde(rename_all = "kebab-case"),
    strum(serialize_all = "kebab-case"),
)]
pub enum ToolScope { ... }
```

Re-export `ToolScopeKind` from `specify_tool` (alongside `ToolScope`). Delete the hand-rolled enum at `src/commands/tool/dto.rs:46-54` and switch the import to `use specify_tool::{Tool, ToolPermissions, ToolScope, ToolScopeKind};`. `scope_labels` keeps its current shape — `ToolScopeKind::from(scope)` is provided by the derive but the explicit `match` is fine to retain since it also extracts `scope_detail`.

- **Quality delta**: −≈9 LOC, −1 type definition, −1 cross-crate duplication; no new dependency.
- **Net LOC**: `dto.rs` 246 → ≈237; `manifest.rs` 322 → ≈329 (one derive block added). Cross-crate net ≈ −2 LOC, but the structural win is the eliminated mirror.
- **Done when**: `rg -c 'enum ToolScopeKind' src/ crates/` → `0`; `rg 'ToolScopeKind' src/ crates/ -t rust` shows only uses (no definition); `cargo make lint` and `cargo test --workspace` clean.
- **Rule?**: No.
- **Counter-argument**: "the mirror keeps `ToolScopeKind` private to the CLI crate; deriving in `specify_tool` widens the public surface." — loses because (a) `ToolScope` itself is already `pub` in `specify_tool`, so the discriminant is a strictly smaller addition, and (b) the derive replaces a hand-maintained enum that has to be updated in lockstep with `ToolScope` variants — the kind of duplication this pass exists to delete.
- **Depends on**: none.

---

## One-touch tidies

### T1. Drop redundant `has_project_yaml` guard in `is_clone_eligible`

- **Evidence**: `src/commands/slice/merge.rs:307-314`:

```307:314:src/commands/slice/merge.rs
fn is_clone_eligible(project_dir: &Path) -> bool {
    if !is_workspace_clone(project_dir) {
        return false;
    }
    let has_project_yaml = project_dir.join(".specify").join("project.yaml").exists();
    let has_plan_yaml = Layout::new(project_dir).plan_path().exists();
    has_project_yaml && !has_plan_yaml
}
```

`scoped()` in `src/commands.rs:90-102` calls `Ctx::load`, which fails fast with `Error::NotInitialized` if `.specify/project.yaml` is missing for the same `project_dir`. By the time `slice merge run` reaches `is_clone_eligible`, `has_project_yaml` is `true` by construction.

- **Action**: delete the `has_project_yaml` line and inline the predicate as `!has_plan_yaml`. The `Layout::new(project_dir).plan_path()` already encapsulates the only discriminating check (`plan.yaml` absent => hub project).
- **Quality delta**: −3 LOC, −1 branch.
- **Net LOC**: 8 → 5.
- **Done when**: `rg 'has_project_yaml' src/commands/slice/merge.rs` → no matches.
- **Rule?**: No.
- **Counter-argument**: "defence in depth against a future caller without `Ctx`" — loses because `is_clone_eligible` is file-private and called once, only by `run()`; if a hypothetical future caller skips `Ctx::load`, the symptom is a panic from a deeper layer, not a missed auto-commit.
- **Depends on**: none.

### T2. Drop now-stale "R4 routes every error envelope" comment

- **Evidence**: `tests/common/mod.rs:210-212`:

```210:213:tests/common/mod.rs
/// Mirror of [`parse_stdout`] for the stderr channel. Used by failure
/// tests since R4 routes every error envelope (JSON or text) through
/// `Stream::Stderr`.
```

`Stream::Stderr` is `output.rs`-private; the comment cites a review-ticket identifier (`R4`) by name. The comment misleads readers grepping for `Stream::Stderr` outside `src/output.rs` (and will be wrong after S2 lands).

- **Action**: rewrite the doc-comment to say "Mirror of [`parse_stdout`] for the stderr channel. Used by failure tests, which write the error envelope to stderr in both JSON and text formats." Drop the `R4` reference and the `Stream::Stderr` name.
- **Quality delta**: −2 LOC (3-line comment → 1-line); fixes an actively wrong reference (allowed under the rules).
- **Net LOC**: 3 → 1.
- **Done when**: `rg 'Stream::Stderr|R4' tests/common/mod.rs` → no matches.
- **Rule?**: No.
- **Counter-argument**: "rule says no comment edits unless misleading" — loses because this comment cites a private type and an internal ticket id; it qualifies as actively wrong under the rules.
- **Depends on**: S2 (otherwise this is a pure rename and the rules forbid that on its own; bundle into the same change).

### T3. Collapse `merge_pathspecs` + first inline `add_args` walk in `auto_commit`

- **Evidence**: `src/commands/slice/merge.rs:316-322` defines `merge_pathspecs` (filter the constant array against `project_dir`); the only caller, `auto_commit` at `:329`, uses the result twice (`add_args`, `diff_args`, `commit_args`). With only two paths in the constant and exactly one call site, the helper trades a 5-line fn for a 1-line `iter().filter(...)` chain inline; the borrow is fine because `pathspecs` is bound once in `auto_commit`.
- **Action**: inline `merge_pathspecs` into `auto_commit` as `let pathspecs: Vec<&str> = WORKSPACE_MERGE_COMMIT_PATHS.iter().copied().filter(|p| project_dir.join(p).exists()).collect();`. Delete `fn merge_pathspecs`.
- **Quality delta**: −≈6 LOC, −1 internal fn boundary.
- **Net LOC**: 13 (helper + first use) → 7.
- **Done when**: `rg 'fn merge_pathspecs' src/commands/slice/merge.rs` → no matches.
- **Rule?**: No.
- **Counter-argument**: "the helper is named and testable" — loses because nothing in `mod tests` exercises `merge_pathspecs` directly (it's file-private), and the inline form fits on one line.
- **Depends on**: none.

### T4. Drop the `#[expect(clippy::too_many_lines, ...)]` cluster in `validate_shape`

- **Evidence**: `crates/domain/src/registry/validate.rs:21-24` — covered by S1; the suppression exists to justify the duplication that S1 deletes.
- **Action**: delete the attribute. Verify with `cargo make lint`.
- **Quality delta**: −4 LOC, −1 lint-override.
- **Net LOC**: 4 → 0.
- **Done when**: `rg 'too_many_lines' crates/domain/src/registry/validate.rs` → no matches and `cargo make lint` is clean.
- **Rule?**: No.
- **Counter-argument**: none — the suppression was conditional on the duplication.
- **Depends on**: S1.

---

## Dropped findings (and why)

- **`Sub` struct + `Sub::new` in `tests/common/mod.rs:138-150`** — newtype over `(String, &'static str)`. Could be a tuple, but the conversion `from: impl Into<String>` keeps every call site `Sub::new("foo", "<TEMPDIR>")` clean; replacing it with `(String, &'static str)` forces `.to_string()` at each call. No LOC win.
- **`From<serde_saphyr::*>` impls in `crates/error/src/error.rs:139-149`** — looked redundant with `YamlError`'s `#[from]` derive. They are necessary because they bridge the *outer* `Error` enum without exposing `serde_saphyr` from public surfaces; deleting them forces `Error::Yaml(YamlError::from(...))` at every call site.
- **`SourceArg` newtype in `src/cli.rs:148-171`** — looked over-shaped, but clap's derive `FromStr` boundary is the smallest form here; replacing with a `(String, String)` would lose the typed error-message path.
- **Skills body-cap drift** — `plugins/omnia/skills/code-reviewer/SKILL.md` (185 lines) and `plugins/spec/skills/analyze/SKILL.md` (168 lines) approach the 200-line cap but are under it; no body-vs-frontmatter drift visible. Not a finding under this pass's threshold.

---

## Post-mortem

- **S2** — predicted −14 LOC, actual −24 LOC in `src/output.rs` (199 → 175). Overshoot because deleting `enum Stream` took its 7-line doc-comment with it, deleting `writer_for` took its 2-line doc-comment, and the `Format::Text` branch collapsed from a 4-line block to a 1-line expression once the per-branch `let mut writer = writer_for(stream);` lines went away. Done-when `rg -c 'enum Stream'` → `0` and `rg -c 'fn writer_for'` → `0` both flipped cleanly. T2 bundled per its `Depends on: S2` annotation; its done-when (`rg 'Stream::Stderr|R4' tests/common/mod.rs` → `0`) also flipped cleanly, but the review under-scoped the cleanup — the same `// R4 routes every error envelope through Stream::Stderr.` zombie comment lived in five additional test files (`tests/cli.rs`, `tests/context.rs`, `tests/change_create.rs`, `tests/change_plan_orchestrate.rs`, `tests/registry.rs`); rewrote all 9 occurrences to the same plain-English form to keep the dead-symbol invariant. AGENTS.md item 4 ("`rg <SymbolName>` AGENTS.md DECISIONS.md docs/ on removal") forced two further doc files to lose their `Stream::Stdout` / `Stream::Stderr` references (`docs/standards/coding-standards.md`, `docs/standards/handler-shape.md`), including one inline code example — uncosted by the review. Cross-file net: +32 / −56 = −24 LOC across 9 files. `cargo make ci` clean; no behavioural regressions; the `Box<dyn Write>` parameter is constructed at the two literal call sites in `write` and `report` exactly as the action spelled out.
- **S3** — predicted −10 LOC, actual −10 LOC (−9 in deleted `crates/domain/src/serde_helpers.rs`, −1 in `crates/domain/src/lib.rs`, 0 net in `config.rs`'s one-for-one attribute swap). Both done-when assertions flipped cleanly: `rg --files crates/domain/src | rg serde_helpers` → no matches, `rg 'is_false' src/ crates/ -t rust` → no matches. AGENTS.md item 4 sweep was a no-op — no `is_false`/`serde_helpers` hits in `AGENTS.md`, `DECISIONS.md`, or `docs/`. `cargo make ci` clean; the `std::ops::Not::not` form serialises `hub: false` as omitted exactly like the named predicate did. No regressions.
- **S1** — predicted −25 LOC, actual −11 LOC in `crates/domain/src/registry/validate.rs` (280 → 269). Three trailing `for project in &self.projects` walks folded into the existing `.iter().enumerate()` shape loop with `producers: HashMap<&str, &str>` declared above. The reviewer under-counted `validate_shape`'s residual size: after the inline collapse the body still measured 106 lines (>100), tripping `clippy::too_many_lines` once T4's suppression was dropped. Resolved by lifting the per-project contract block into a `validate_project_contracts(project, &mut producers)` helper — preserves the "1 loop, 1 source of truth for contract rules" structural win and the `−1 clippy override` quality delta, but costs ~13 LOC vs the inline prediction. Done-when `rg -c 'for project in &self.projects'` → `0` (the review predicted `1`; the kept shape pass uses `.iter().enumerate()`, so the pattern only ever matched the three deleted walks — the assertion still flipped cleanly in spirit, going from 3 to 0). `rg 'too_many_lines'` → `0`. Diagnostic-ordering shift (contract errors for project[N] now precede shape errors for project[N+1] instead of all-shape-then-all-contracts) — accepted per the "most likely to break" callout; no tests pin the cross-project ordering. T4 satisfied as part of the same edit. `cargo make ci` clean.
