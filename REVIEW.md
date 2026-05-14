# Code & Skill Review — `specify` + `specify-cli` (second pass)

## Summary

- **Top three by LOC**: S1 collapses `PackageSnapshot` + `OciSnapshot` + `PermissionsSnapshot` mirror types into the underlying domain types (~50 LOC); S2 drops the `Collision` single-field newtype (~10 LOC); S3 drops `CachedCapability` newtype (~6 LOC).
- **Total ΔLOC if all 4 structurals + 6 tidies land**: ≈ **−115 LOC**, mostly in `crates/tool/src/cache/meta.rs`, `crates/tool/src/load.rs`, `crates/tool/src/validate.rs`, and the test fixtures under `crates/tool/src/`.
- **Non-LOC axes moved**: −5 types, −5 `From` impls, −1 wire-shape collapse (`oci.reference` folds into `package.oci-reference`), −2 helper functions.
- **Most likely to break**: S1 — moving `oci_reference` from a sibling DTO into `PackageMetadata` flips the `meta.yaml` wire shape and the `tool show` JSON envelope; tests under `tests/tool.rs` and `tests/contract_tool.rs` assert on `oci.reference` and need the snapshot updated in lockstep.
- **Reconnaissance**: 48 704 Rust LOC in `specify-cli`, 271 files; **no** `mod.rs` outside `tests/` (already disciplined); standards docs total **498 lines** under `docs/standards/`; one `cargo tree --duplicates` cluster (warg/base64/oci, already on the `clippy.toml` allow-list); `crates/tool/src/validate.rs` (511) and `crates/tool/src/package.rs` (503) are the only non-test sources >500 LOC and both stay under the 600-line tripwire.

---

## Structural findings

### S1. Collapse `PackageSnapshot` + `OciSnapshot` + `PermissionsSnapshot` mirror types

**Evidence**: Three sidecar DTOs in `crates/tool/src/cache/meta.rs` are byte-shape-identical mirrors of types that already live one module away.

```rust
// crates/tool/src/cache/meta.rs:18-27
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct PermissionsSnapshot {
    #[serde(default)]
    pub read: Vec<String>,
    #[serde(default)]
    pub write: Vec<String>,
}
```

```rust
// crates/tool/src/cache/meta.rs:30-47
pub struct PackageSnapshot {
    pub name: String,
    pub version: String,
    pub registry: String,
}
// ...
pub struct OciSnapshot {
    pub reference: String,
}
```

```rust
// crates/tool/src/cache/meta.rs:49-66
impl From<&PackageMetadata> for PackageSnapshot { /* clones 3 fields */ }
impl From<&ToolPermissions> for PermissionsSnapshot { /* clones 2 fields */ }
```

Current-state grep:

```
$ rg "struct (PackageSnapshot|OciSnapshot|PermissionsSnapshot)|impl From<&(PackageMetadata|ToolPermissions)>" crates/tool/src/cache/meta.rs
20:pub struct PermissionsSnapshot {
32:pub struct PackageSnapshot {
44:pub struct OciSnapshot {
49:impl From<&PackageMetadata> for PackageSnapshot {
59:impl From<&ToolPermissions> for PermissionsSnapshot {
```

`PermissionsSnapshot { read, write }` is byte-identical to `ToolPermissions { read, write }` (kebab-case is a no-op on single-word fields). `PackageSnapshot { name, version, registry }` is `PackageMetadata { name, version, registry, oci_reference: Option<String> }` minus one optional field. `OciSnapshot { reference }` exists solely so `oci_reference` can serialize as a sibling block on the sidecar.

**Action**:

1. In `crates/tool/src/package.rs`, add `serde::{Serialize, Deserialize}` derives + `#[serde(rename_all = "kebab-case", deny_unknown_fields)]` to `PackageMetadata`. Keep `#[serde(default, skip_serializing_if = "Option::is_none")]` on `oci_reference`.
2. In `crates/tool/src/cache/meta.rs`:
   - Delete `struct PackageSnapshot`, `struct OciSnapshot`, `struct PermissionsSnapshot`, the two `From` impls, and the `Sidecar.oci: Option<OciSnapshot>` field.
   - Change `Sidecar.permissions_snapshot: PermissionsSnapshot` to `permissions_snapshot: ToolPermissions`.
   - Change `Sidecar.package: Option<PackageSnapshot>` to `package: Option<PackageMetadata>`.
   - Replace the `package_metadata.map_or((None, None), |metadata| ...)` block in `Sidecar::new` with `package: package_metadata`.
   - Fold the `if let Some(oci)` non-empty check into the existing `if let Some(package)` branch in `validate_sidecar_schema`.
3. In `crates/tool/src/cache.rs`, drop `OciSnapshot, PackageSnapshot, PermissionsSnapshot` from the `pub use` re-export.
4. In `crates/tool/src/resolver.rs:127`, replace `PermissionsSnapshot::from(&tool.permissions)` with `tool.permissions.clone()`.
5. In `crates/tool/src/cache/tests.rs:28`, replace `PermissionsSnapshot { ... }` with `ToolPermissions { ... }`.
6. In `src/commands/tool/dto.rs`:
   - Drop `OciSnapshot, PackageSnapshot` from the `use` and remove the `oci` field on `ToolShowRow`.
   - Render `package.oci_reference` directly inside the `if let Some(package) = ...` branch in `write_show_text`.
   - In `show_row_for`, drop `let oci = sidecar.as_ref().and_then(...)`.
7. Update `tests/tool.rs` golden assertions on `value["oci"]["reference"]` → `value["package"]["oci-reference"]`.

**Quality delta**: −50 LOC, −3 types, −2 `From` impls, −1 wire shape (`Sidecar.oci` folds into `package.oci-reference`), −1 sidecar struct field, −1 destructure block.
**Net LOC**: meta.rs 252 + cache.rs 222 + dto.rs 236 + resolver.rs 330 + tests/tool.rs assertions → ≈ **−50 net**.
**Done when**: `rg "struct (PackageSnapshot|OciSnapshot|PermissionsSnapshot)|impl From<&(PackageMetadata|ToolPermissions)>" crates/tool/src/` returns nothing; `cargo nextest run -p specify-tool` and `cargo nextest run --test tool --test contract_tool` are green; `specify tool show ... --format json | jq '.tool.package."oci-reference"'` resolves.
**Rule?**: No.
**Counter-argument**: "Snapshot types isolate the wire schema from internal domain churn." Loses: the wire is pinned by `tool-sidecar.schema.json`, not by Rust struct identity; `PackageMetadata` is itself `pub` and crate-stable. Cargo and ripgrep both serialize their domain types directly (`cargo::core::Package`, `ripgrep::Args`) rather than maintaining mirror DTOs.
**Depends on**: none.

---

### S2. Replace `Collision` newtype struct with `String`

**Evidence**:

```rust
// crates/tool/src/load.rs:9-15
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Collision {
    /// Colliding tool name.
    pub name: String,
}
```

```rust
// src/commands/tool/dto.rs:222-231
pub(super) fn warning_row(collision: Collision) -> WarningRow {
    let Collision { name } = collision;
    WarningRow { code: "tool-name-collision", message: format!("..."), name }
}
```

Current-state grep:

```
$ rg "Collision \{|struct Collision" crates/tool/ src/
crates/tool/src/load.rs:12:pub struct Collision {
crates/tool/src/load.rs:70:            warnings.push(Collision { name: tool.name });
crates/tool/src/load.rs:177:            vec![Collision {
src/commands/tool/dto.rs:223:    let Collision { name } = collision;
```

One field, no methods, no invariants, two constructions, one destructure. Pure shape ceremony.

**Action**:

1. In `crates/tool/src/load.rs`, delete `pub struct Collision`. Change `merge_scoped`'s return type from `Vec<Collision>` to `Vec<String>`. Replace `warnings.push(Collision { name: tool.name })` with `warnings.push(tool.name)`.
2. Update the `merge_scoped_project_wins_and_warns_once` test: `vec!["contract".to_string()]`.
3. In `src/commands/tool/dto.rs`, change `warning_row(collision: Collision)` to `warning_row(name: String)` and inline without the destructure. Update the call site in `commands/tool/list.rs`.

**Quality delta**: −10 LOC, −1 public type, −1 destructure block, −1 module edge.
**Net LOC**: load.rs 188 + dto.rs 236 → ≈ **−10 net**.
**Done when**: `rg "Collision" crates/tool/ src/` returns nothing; `cargo nextest run -p specify-tool load::tests` is green.
**Rule?**: No.
**Counter-argument**: "`Collision` is more discoverable than a `String` in `Vec<String>`." Loses: discoverability lives in the function name `warning_row`, not the parameter type. `cargo`'s `core::compiler::warnings` tracks warnings as `Vec<String>` for the same reason.
**Depends on**: none.

---

### S3. Drop `CachedCapability` single-field newtype

**Evidence**:

```rust
// crates/domain/src/init/cache.rs:15-42
#[derive(Debug)]
pub(super) struct CachedCapability {
    pub(crate) capability_value: String,
}

pub(super) fn cache_capability(...) -> Result<CachedCapability, Error> {
    // ...
    Ok(CachedCapability { capability_value: source.capability_value })
}
```

```rust
// crates/domain/src/init/regular.rs:50-66
let resolved = cache_capability(capability, opts.project_dir, now)?;
let view = PipelineView::load(&resolved.capability_value, opts.project_dir)?;
// ...
capability: Some(resolved.capability_value),
```

Current-state grep:

```
$ rg "struct CachedCapability|CachedCapability \{|resolved\.capability_value" crates/domain/src/init/
crates/domain/src/init/cache.rs:16:pub(super) struct CachedCapability {
crates/domain/src/init/cache.rs:39:    Ok(CachedCapability {
crates/domain/src/init/regular.rs:52:    let view = PipelineView::load(&resolved.capability_value, opts.project_dir)?;
crates/domain/src/init/regular.rs:66:        capability: Some(resolved.capability_value),
```

One field, no methods, no `Drop`, no invariants. The wrapper is paying nothing.

**Action**:

1. In `crates/domain/src/init/cache.rs`, delete `pub(super) struct CachedCapability`. Change `cache_capability` return type from `Result<CachedCapability, Error>` to `Result<String, Error>`. Replace the tail with `Ok(source.capability_value)`.
2. In `crates/domain/src/init/regular.rs`, rename `let resolved = ...` to `let capability_value = ...` and replace the two field accesses.

**Quality delta**: −6 LOC, −1 type, −1 derive (`#[derive(Debug)]`), −1 field-access pattern.
**Net LOC**: cache.rs 95 + regular.rs 351 → ≈ **−6 net**.
**Done when**: `rg "CachedCapability" crates/domain/src/init/` returns nothing.
**Rule?**: No.
**Counter-argument**: "Future fields might join `capability_value`." Loses: when the second field appears, re-introduce the struct then; YAGNI now. Cargo's resolver returns bare `String` ids for the same use case.
**Depends on**: none.

---

### S4. Collapse `pass`/`fail`/`check` triple in `crates/tool/src/validate.rs`

**Evidence**:

```rust
// crates/tool/src/validate.rs:194-216
fn pass(rule_id: &'static str, rule: &'static str) -> ValidationSummary { /* 7 LOC */ }
fn fail(rule_id: &'static str, rule: &'static str, detail: impl Into<String>) -> ValidationSummary { /* 7 LOC */ }
fn check(rule_id: &'static str, rule: &'static str, valid: bool, detail: impl FnOnce() -> String) -> ValidationSummary {
    if valid { pass(rule_id, rule) } else { fail(rule_id, rule, detail()) }
}
```

`pass` and `fail` only have three callers each — the three `validate_*` helpers, each wrapping the same `if failures.is_empty() { pass(...) } else { fail(..., failures.join("; ")) }` shape. The `(bool, String)` tuple pattern earlier in `validate_structure` is the same idea inside-out — pre-compute a `(valid, detail)` pair, pass both to `check`, reconstruct internally.

**Action**:

1. Delete `pass` and `fail`. Rewrite `check` to take `Option<String>` (None = pass, Some = fail).
2. Rewrite the three `(bool, String)` tuples in `validate_structure` (lines 71-105) as `Option<String>`:
   ```rust
   let package_namespace_detail = package.and_then(|p| {
       (p.namespace != "specify").then(|| format!("`{}` is not in the specify namespace", p.name_ref()))
   });
   ```
3. Update the eleven `check(rule, "...", valid, || detail)` call sites to pass `Option<String>` directly.
4. In the three `validate_*` helpers, replace the if/else tail with `check(RULE_X, RULE, (!failures.is_empty()).then(|| failures.join("; ")))`.

**Quality delta**: −20 LOC, −2 helper functions, −3 `(bool, String)` tuple bindings, −1 branch.
**Net LOC**: validate.rs 511 → ≈ **−20 net**.
**Done when**: `rg "^fn (pass|fail)\(" crates/tool/src/validate.rs` returns nothing; `cargo nextest run -p specify-tool validate::tests` is green; the `validate_structure` impl shrinks below `clippy::too_many_lines` (currently `#[expect]`-silenced) — verify by removing the `#[expect]`.
**Rule?**: No.
**Counter-argument**: "`pass`/`fail` read more declaratively." Loses: `Option<String>` is the canonical "pass-or-explain-the-failure" idiom in std (`Result::ok` / `Option::then`); ripgrep's `args::ArgsImpl::validate` uses the same shape.
**Depends on**: none.

---

## One-touch tidies (each < 30 LOC, single-axis)

### T1. Drop the `cache-env` test-setup quad with one helper in `test_support`

`crates/tool/src/resolver.rs`, `resolver/digest.rs`, `resolver/http.rs`, `resolver/local.rs`, `cache/tests.rs` repeat the exact 4-line setup eleven times:

```rust
let _g = env_lock();
let _cache = EnvGuard::set("SPECIFY_TOOLS_CACHE", &cache_dir);
let _xdg = EnvGuard::unset("XDG_CACHE_HOME");
let _home = EnvGuard::unset("HOME");
```

Add one helper in `crates/tool/src/lib.rs::test_support` returning `(MutexGuard<'static, ()>, [EnvGuard; 3])` so each call site collapses to `let _env = test_support::cache_env(&cache_dir);`. Drop order is correct: the array drops first (guards restore env), then the lock drops last. **Delta**: −22 LOC. **Done when**: `rg "EnvGuard::unset\(\"HOME\"\)" crates/tool/src/` returns exactly one match (inside the helper).

### T2. Drop `WasmPkgClient::default()` derive and unwrap `Option<PathBuf>`

`crates/tool/src/package.rs:110-122`. `Default` has zero callers (`rg "WasmPkgClient::default|WasmPkgClient \{ \.\.Default" crates/ src/` is empty). The single `WasmPkgClient::new(...)` site at `resolver.rs:51` always passes `Some(project_dir.to_path_buf())`. Remove `Default` and the `Option` collapses; `fetch` swaps `self.project_dir.as_deref()` for `Some(&self.project_dir)`. **Delta**: −4 LOC. **Done when**: `rg "Option<PathBuf>" crates/tool/src/package.rs` returns nothing.

### T3. Hoist the duplicated lowercase-hex SHA-256 predicate

```rust
// crates/tool/src/cache/meta.rs:249-251
fn valid_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
```

```rust
// crates/tool/src/validate.rs:107-109
let sha256_valid = self.sha256.as_deref().is_none_or(|v| {
    v.len() == 64 && v.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
});
```

Hoist to `crates/tool/src/manifest.rs::looks_like_sha256_hex` next to the existing `looks_like_windows_absolute`. **Delta**: −2 LOC. **Done when**: `rg "\(b'a'\.\.=b'f'\)" crates/tool/src/` returns one match (the hoisted helper).

### T4. Drop the `debug_assert!` xor check in `src/commands/init.rs`

```rust
// src/commands/init.rs:26-29
debug_assert!(
    hub != capability.is_some(),
    "clap enforces <capability> xor --hub; reached dispatcher with hub={hub}, capability={capability:?}",
);
```

Clap enforces the xor via `#[arg(conflicts_with = ...)]`; `domain::init::run` re-validates and emits `init-requires-capability-or-hub` for any caller that bypasses clap. The dispatcher `debug_assert!` is defence in depth on top of two real defences. **Delta**: −4 LOC. **Done when**: `rg "debug_assert.*hub.*capability" src/` returns nothing; `tests/init.rs::init_requires_capability_or_hub` still asserts the diagnostic.

### T5. Inline the single-call-site `path_to_env` helper in `crates/tool/src/host.rs`

```rust
// crates/tool/src/host.rs:257-265
fn path_to_env<'a>(path: &'a Path, name: &str) -> Result<&'a str, ToolError> {
    path.to_str().ok_or_else(|| ToolError::invalid_permission(name, format!("...")))
}
```

Two callers (`build_wasi_ctx` lines 233 and 235), both within the same function within ten lines of the helper. The four-line ladder per call site is exactly as long as inlining the body. **Delta**: −7 LOC. **Done when**: `rg "fn path_to_env|path_to_env\(" crates/tool/src/host.rs` returns nothing.

### T6. Inline `Sidecar::new` at its single call site

After S1 lands, `Sidecar::new`'s argument list is still 8, so the `#[expect(clippy::too_many_arguments, ...)]` stays. But the function is constructed at exactly one site (`resolver.rs::stage_and_install`); the only behaviour beyond field-assignment is `scope_segment(scope)?`. Inline `scope_segment` at the call site, write the struct literal directly, drop `Sidecar::new` and its `#[expect]`. **Delta**: −15 LOC, −1 method, −1 lint suppression. **Done when**: `rg "fn new\(" crates/tool/src/cache/meta.rs` returns nothing.

---

## Dropped during review (with reasons)

- **"Replace `unique_temp_dir` in `crates/domain/src/init/git.rs:57-68` with `tempfile::Builder::new().tempdir()`"** — `tempfile` is not in `crates/domain`'s `Cargo.toml`; adding it is a Cargo edge that the master rule freezes for this pass.
- **"Promote `validate_*` helpers to a single `validate_with_failures` macro"** — would add a `macro_rules!` for three call sites; not strictly smaller than the inline `Vec<String>` + `if failures.is_empty()` pattern.
- **"Split `crates/tool/src/validate.rs` (511 LOC)"** — under the 600-line tripwire; module-split adds a new file, forbidden by the master rule.
- **"Drop `tool-host-not-built` stub `WasiRunner`"** — same finding raised in the previous review pass and dropped for the same reason: changes the public crate surface for any consumer that builds `specify-tool` without the `host` feature.
- **"Combine `has_parent_segment` in `permissions.rs` and `validate.rs`"** — semantics differ (`permissions.rs` also checks `Component::ParentDir` via `Path::components`); merging would change behaviour.
- **"Trim `analyze/SKILL.md` (168 lines, well under the 200-line cap)"** — comment/prose-edit-only finding without an LOC-cap violation; master rule rejects taste-only skill trims.
- **"Replace `out.push_str(...)` ladder in `src/commands/context/render.rs::render_document_with_fingerprint` with a single `write!` block"** — formatting-only; LOC neutral after rustfmt.
- **"Drop the `pub use specify_tool::{Tool, ToolManifest, ...}` re-export in `crates/tool/src/lib.rs:20`"** — the binary's `commands/tool/dto.rs` reaches through this surface; removing it just moves imports without a net LOC change.

---

## Post-mortem

- **S1**: predicted −50 LOC, actual **−72 LOC** (30 insertions, 102 deletions across 8 files); done-when flipped clean (`rg "struct (PackageSnapshot|OciSnapshot|PermissionsSnapshot)|impl From<&(PackageMetadata|ToolPermissions)>" crates/tool/src/` empty; 837/837 workspace tests green including `cache_miss_hit_and_override_observable`, `package_source_uses_injected_client_and_records_metadata`, `contract_tool`, `vectis_tool`); no regressions. Calibration: prediction undercounted by ~22 LOC because the doc-comments on the deleted `///`-prefixed snapshot fields and the schema-file collapse weren't in the LOC envelope; also pre-empted the prior-session `private_interfaces` trap by adding `pub use package::PackageMetadata` to `lib.rs` in the same edit (the action list didn't call this out, so future S-grade collapses that surface a private-mod type via a pub struct field should pre-add the re-export).
