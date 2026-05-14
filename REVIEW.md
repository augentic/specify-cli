# Code & Skill Review — `specify` + `specify-cli`

## Summary

- **Top three by LOC**: S1 EnvGuard collapses `with_cache_env` (≈40 LOC), S2 drops `Stdio`/`with_stdio` test-only plumbing (≈25 LOC), S3 inlines `ContextBody` into `Body` (≈22 LOC).
- **Total ΔLOC if all 8 structural findings land**: ≈ **−175 LOC** in `crates/tool/` and `src/commands/init.rs`; collapses 4 mirror/sentinel types and 2 duplicate helper triples.
- **Non-LOC axes moved**: −4 types, −5 branches in `Stdio` + `ContextGeneration` dispatch, −1 trait return shape (`PackageClient::fetch`), −3 duplicate fn copies.
- **Most likely to break in remediation**: S1 — the `with_cache_env` → `EnvGuard` port must touch every cache/resolver test sharing process-wide `SPECIFY_TOOLS_CACHE` / `XDG_CACHE_HOME` / `HOME`; getting drop order wrong silently leaks env vars across tests.
- Reconnaissance: 49 099 Rust LOC; 70 test files; **no** `mod.rs` outside `tests/` (already disciplined); 3 standards docs (88 + 225 + 68 = **381** lines under `docs/standards/`); `cargo tree --duplicates` shows the duplicate set already curated in `clippy.toml::allowed-duplicate-crates`.

---

## Structural findings

### S1. Replace `with_cache_env` closure with RAII `EnvGuard`

**Evidence**: Two implementations of "snapshot a process env var, mutate under `env_lock`, restore on exit" live in the same crate.

```rust
// crates/tool/src/lib.rs:104-146
pub fn with_cache_env<T>(
    specify_cache: Option<&Path>, xdg_cache: Option<&Path>, home: Option<&Path>,
    f: impl FnOnce() -> T,
) -> T { ... }

fn set_or_remove_env(key: &str, value: Option<&Path>) { ... }
fn restore_env(key: &str, value: Option<std::ffi::OsString>) { ... }
```

```rust
// crates/tool/src/package.rs:315-347
struct EnvGuard {
    key: &'static str,
    previous: Option<std::ffi::OsString>,
}

impl EnvGuard {
    fn set(key: &'static str, value: &Path) -> Self { ... }
    fn unset(key: &'static str) -> Self { ... }
}

impl Drop for EnvGuard { ... }
```

Current-state grep: `rg "struct EnvGuard|with_cache_env" crates/tool/src/` reports **two** distinct env-mutation primitives in the same crate. `EnvGuard` is the strict subset: every `with_cache_env(Some(a), None, None, || { ... })` call site is one `let _g = EnvGuard::set("SPECIFY_TOOLS_CACHE", a);` line; `None` arms become `EnvGuard::unset(...)`.

**Action**:

1. Move `EnvGuard` from `crates/tool/src/package.rs` into `crates/tool/src/lib.rs::test_support` next to `env_lock`.
2. Delete `with_cache_env`, `set_or_remove_env`, `restore_env`.
3. Port each `with_cache_env(Some(&cache_dir), None, None, || { ... })` call (`resolver.rs`, `resolver/digest.rs`, `resolver/local.rs`, `resolver/http.rs`, `cache/tests.rs`) to:

   ```rust
   let _g = env_lock();
   let _cache = EnvGuard::set("SPECIFY_TOOLS_CACHE", &cache_dir);
   let _xdg = EnvGuard::unset("XDG_CACHE_HOME");
   let _home = EnvGuard::unset("HOME");
   // ... test body ...
   ```

**Quality delta**: −40 LOC, −1 helper trio (`set_or_remove_env` + `restore_env` + `with_cache_env`), −1 closure indent level at every call site.
**Net LOC**: lib.rs 147 + package.rs 549 + ~6 resolver test files → roughly **≈ −40 net**.
**Done when**: `rg "with_cache_env|set_or_remove_env|restore_env" crates/tool/` returns nothing; `cargo nextest run -p specify-tool` is green.
**Rule?**: No — the duplicate is a one-time accumulation, not a recurring pattern.
**Counter-argument**: "Closure scoping is more obviously bounded than RAII." Loses: every other env-snapshot guard in std (`tempfile::TempDir`, `std::sync::MutexGuard`) is RAII, and the call-site delta is monotonically smaller (one line per var vs. nested closure).
**Depends on**: none.

---

### S2. Drop `Stdio` / `with_stdio` test-only plumbing

**Evidence**:

```rust
// crates/tool/src/host.rs:31-78
pub enum Stdio {
    #[default]
    Inherit,
    Null,
}

pub struct RunContext {
    ...
    pub stdio: Stdio,
}

impl RunContext {
    pub const fn with_stdio(mut self, stdio: Stdio) -> Self { ... }
}
```

Current-state grep: `rg "Stdio::Null|with_stdio"` reports **3 hits, all in `crates/tool/src/host.rs::tests`**. Production callers (`src/commands/tool/run.rs:23`) use `RunContext::new(...)` with default `Stdio::Inherit` and never call `with_stdio`. `Stdio::Null` exists exclusively so three tests can pass `RunContext::new(...).with_stdio(Stdio::Null)` to avoid spamming captured stderr — but `cargo nextest` already captures stderr, so the variant is buying nothing.

**Action**:

1. Delete `pub enum Stdio`, the `stdio` field on `RunContext`, the `with_stdio` method, and the `match ctx.stdio { Inherit => builder.inherit_stdio(), Null => {} }` block in `build_wasi_ctx` — replace with a single unconditional `builder.inherit_stdio();`.
2. Remove `.with_stdio(Stdio::Null)` from the three host.rs tests.

**Quality delta**: −25 LOC, −1 enum, −1 method, −1 struct field, −1 match arm.
**Net LOC**: host.rs 457 → ≈432.
**Done when**: `rg "Stdio|with_stdio" crates/tool/src/host.rs` is empty; `cargo nextest run -p specify-tool host::tests` is green.
**Rule?**: No.
**Counter-argument**: "Future runs might want quiet stdio." Loses: pre-1.0, a YAGNI knob that only the tests touch is dead weight; re-add when the second caller appears.
**Depends on**: none.

---

### S3. Inline `ContextBody` fields into `Body` (drop the nested DTO + `#[serde(flatten)]`)

**Evidence**:

```rust
// src/commands/init.rs:46-120
#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct Body {
    ...
    #[serde(flatten)]
    context: ContextBody,
}

#[derive(Serialize)]
struct ContextBody {
    #[serde(rename = "context-generated")]
    generated: bool,
    #[serde(rename = "context-skipped")]
    skipped: bool,
    #[serde(rename = "context-skip-reason", skip_serializing_if = "Option::is_none")]
    skip_reason: Option<&'static str>,
}

impl From<ContextGeneration> for ContextBody {
    fn from(context_generation: ContextGeneration) -> Self { ... }
}
```

`Body` already has `#[serde(rename_all = "kebab-case")]`, so `context_generated` / `context_skipped` / `context_skip_reason` fields on `Body` serialise identically without `#[serde(flatten)]` or the explicit rename trio. The nested DTO + `From` impl exists purely to namespace three fields.

**Action**:

1. Lift the three fields directly onto `Body` (rename `generated` → `context_generated` etc., let kebab-case derive do the wire shape).
2. Delete `struct ContextBody`, the `From<ContextGeneration>` impl, and the `#[serde(flatten)] context: ContextBody` field.
3. Build the fields directly inside `emit_init_result`.

**Quality delta**: −22 LOC, −1 DTO type, −1 `From` impl, −3 `#[serde(rename = ...)]` attributes, −1 `#[serde(flatten)]`.
**Net LOC**: init.rs 189 → ≈167.
**Done when**: `rg "ContextBody|#\[serde\(flatten\)" src/commands/init.rs` is empty; `cargo nextest run init` is green; `specify init … --format json | jq '.["context-generated"]'` still resolves.
**Rule?**: No.
**Counter-argument**: "`ContextBody` is reusable." Loses: it has one call site and one impl; nothing to reuse.
**Depends on**: S4 (do them in the same PR so the `Body` build site is touched once).

---

### S4. Replace `ContextGeneration` enum with `Option<&'static str>`

**Evidence**:

```rust
// src/commands/init.rs:140-157
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ContextGeneration {
    Generated,
    Skipped { reason: &'static str },
}

impl ContextGeneration {
    const fn skip_reason(&self) -> Option<&'static str> { ... }
    const fn skipped(&self) -> bool { ... }
}
```

The enum carries the same information as `Option<&'static str>` (`None` = generated, `Some(reason)` = skipped). Both methods are pure projections of that `Option`. The `matches!(..., Generated)` site in `ContextBody::from` is `option.is_none()`.

**Action**:

1. Change `generate_initial_context` return type to `Result<Option<&'static str>>`.
2. Replace `Ok(ContextGeneration::Generated)` with `Ok(None)`, `Ok(ContextGeneration::Skipped { reason })` with `Ok(Some(reason))`.
3. Delete the enum + impl block.
4. At the build site (after S3) write `context_skip_reason: skip_reason, context_skipped: skip_reason.is_some(), context_generated: skip_reason.is_none()`.

**Quality delta**: −20 LOC, −1 enum, −1 impl block, −2 helper methods.
**Net LOC**: init.rs (already shrinking via S3) → another ≈ −20.
**Done when**: `rg "ContextGeneration" src/` is empty; `cargo nextest run init` green; JSON envelope shape unchanged (compared with `tests/init.rs` snapshots).
**Rule?**: No.
**Counter-argument**: "The named enum reads better than `Option<&str>`." Loses: a two-state enum where one state is the absence of data is exactly what `Option` was designed for; the named alternative just costs more lines.
**Depends on**: paired with S3.

---

### S5. Collapse three duplicate `looks_like_windows_absolute*` helpers in `crates/tool/`

**Evidence**:

```rust
// crates/tool/src/manifest.rs:245-251
fn looks_like_windows_absolute_path(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'\\' | b'/')
}
```

```rust
// crates/tool/src/permissions.rs:178-184
fn looks_like_windows_absolute_path(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'\\' | b'/')
}
```

```rust
// crates/tool/src/validate.rs:323-333
fn path_looks_windows_absolute(path: &Path) -> bool {
    path.to_str().is_some_and(looks_like_windows_absolute_str)
}

fn looks_like_windows_absolute_str(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'\\' | b'/')
}
```

Three byte-for-byte copies of the same 6-line byte check in the same crate.

**Action**:

1. In `crates/tool/src/manifest.rs`, change the function to `pub(crate) fn looks_like_windows_absolute(value: &str) -> bool` (drop `_path` suffix per coding-standards.md naming rules — context already says `windows_absolute`).
2. In `permissions.rs` and `validate.rs`, replace the duplicate definition with `use crate::manifest::looks_like_windows_absolute;` and inline the `path.to_str().is_some_and(looks_like_windows_absolute)` wrapper at the one validate.rs call site (line 49).

**Quality delta**: −16 LOC, −2 duplicate fns, −1 wrapper fn (`path_looks_windows_absolute`).
**Net LOC**: manifest.rs + permissions.rs + validate.rs combined → ≈ −16.
**Done when**: `rg "fn looks_like_windows_absolute|fn path_looks_windows_absolute" crates/tool/src/` returns exactly one line.
**Rule?**: No (already only 3 sites, all in one crate; a clippy lint here would be sandcastling).
**Counter-argument**: "The wrapper documents intent." Loses: a one-line `path.to_str().is_some_and(looks_like_windows_absolute)` documents itself.
**Depends on**: none.

---

### S6. Collapse `FetchedPackage` mirror into `AcquiredBytes`

**Evidence**:

```rust
// crates/tool/src/package.rs:60-80
pub struct FetchedPackage {
    pub temp: NamedTempFile,
    pub sha256: String,
    pub metadata: PackageMetadata,
}

pub trait PackageClient {
    fn fetch(
        &self, request: &PackageRequest, dest_hint: &Path,
    ) -> Result<FetchedPackage, ToolError>;
}
```

```rust
// crates/tool/src/resolver.rs:195-200
pub(crate) struct AcquiredBytes {
    pub(crate) temp: NamedTempFile,
    pub(crate) sha256: String,
    pub(crate) package_metadata: Option<PackageMetadata>,
}
```

```rust
// crates/tool/src/resolver.rs:155-166
ToolSource::Package(package) => package_client.fetch(package, dest_hint).map(
    |FetchedPackage {
         temp,
         sha256,
         metadata,
     }| AcquiredBytes {
        temp,
        sha256,
        package_metadata: Some(metadata),
    },
),
```

`FetchedPackage` exists solely so `acquire_source_bytes` can destructure it and rebuild an `AcquiredBytes` with `Some(metadata)`. `PackageClient` is in a private module (`mod package;` in lib.rs), so `AcquiredBytes` can be returned directly without leaking it out of the crate.

**Action**:

1. Move `pub(crate) struct AcquiredBytes` from `resolver.rs` into `package.rs`; keep `pub(crate) impl AcquiredBytes` next to it.
2. Change `PackageClient::fetch` signature to `Result<AcquiredBytes, ToolError>`.
3. Update `WasmPkgClient::fetch` (package.rs:190) to build `AcquiredBytes { temp, sha256: ..., package_metadata: Some(PackageMetadata { ... }) }` directly.
4. In `resolver.rs::acquire_source_bytes`, replace the destructure-and-rebuild block with `ToolSource::Package(package) => package_client.fetch(package, dest_hint),`.
5. Update the `MockPackageClient` in `resolver.rs::tests` (lines 259-282) and delete `use crate::package::{FetchedPackage, ...};` imports.
6. Delete `pub struct FetchedPackage`.

**Quality delta**: −18 LOC, −1 public-facing type, −1 destructure block.
**Net LOC**: package.rs 549 + resolver.rs 376 → ≈ −18.
**Done when**: `rg "FetchedPackage" crates/tool/` returns nothing; `cargo nextest run -p specify-tool` green.
**Rule?**: No.
**Counter-argument**: "Separating `fetch` result from internal `AcquiredBytes` keeps the trait clean." Loses: the trait is crate-internal already; the separation is imaginary.
**Depends on**: pairs with S7.

---

### S7. Drop `AcquiredBytes::sha256_hex` / `package_metadata` getters

**Evidence**:

```rust
// crates/tool/src/resolver.rs:202-217
impl AcquiredBytes {
    pub(crate) fn len(&self) -> Result<u64, ToolError> { ... }

    pub(crate) fn sha256_hex(&self) -> String {
        self.sha256.clone()
    }

    pub(crate) fn package_metadata(&self) -> Option<PackageMetadata> {
        self.package_metadata.clone()
    }

    pub(crate) fn persist_to(self, dest: &Path) -> Result<(), ToolError> { ... }
}
```

`AcquiredBytes::sha256` and `AcquiredBytes::package_metadata` are already `pub(crate)` fields (line 197-199); the getters do a useless clone on read. Both call sites (resolver.rs:121, digest.rs:36) immediately consume the cloned value.

**Action**:

1. Delete `fn sha256_hex(&self)` and `fn package_metadata(&self)`.
2. At resolver.rs:121, replace `acquired.package_metadata()` with `acquired.package_metadata.clone()` (only consumer; one clone is the same cost).
3. At digest.rs:36, replace `acquired.sha256_hex()` with `acquired.sha256.as_str()` (no clone needed — comparison only).

**Quality delta**: −10 LOC, −2 trivial methods, −1 forced clone on the digest comparison hot path.
**Net LOC**: resolver.rs 376 → ≈ −10.
**Done when**: `rg "sha256_hex|fn package_metadata" crates/tool/src/resolver.rs` returns nothing.
**Rule?**: No.
**Counter-argument**: "Methods are more refactor-friendly than field reads." Loses: the type is `pub(crate)`; refactoring it costs the same either way.
**Depends on**: S6 (touch the type once).

---

### S8. Deduplicate `fixed_now()` in `crates/domain/src/init*`

**Evidence**:

```rust
// crates/domain/src/init.rs:178-180
fn fixed_now() -> Timestamp {
    "2026-05-07T00:00:00Z".parse().expect("fixed test stamp")
}
```

```rust
// crates/domain/src/init/regular.rs:106-108
fn fixed_now() -> Timestamp {
    "2026-05-07T00:00:00Z".parse().expect("fixed test stamp")
}
```

```rust
// crates/domain/src/init/hub.rs:148-150
fn fixed_now() -> Timestamp {
    "2026-05-07T00:00:00Z".parse().expect("fixed test stamp")
}
```

Three byte-identical copies in the same module subtree. (A fourth identical body lives in `crates/tool/src/lib.rs::test_support`.)

**Action**:

1. In `crates/domain/src/init.rs`, hoist the test helper out of `#[cfg(test)] mod tests` to module scope as `#[cfg(test)] pub(super) fn fixed_now() -> Timestamp { ... }`.
2. In `init/regular.rs::tests` and `init/hub.rs::tests`, replace the local definition with `use super::super::fixed_now;`.

**Quality delta**: −6 LOC; tiny but reduces three copies to one.
**Net LOC**: init.rs + init/regular.rs + init/hub.rs combined → ≈ −6.
**Done when**: `rg "fn fixed_now" crates/domain/src/init` returns exactly one match.
**Rule?**: No.
**Counter-argument**: "Each test module should own its fixtures." Loses: the three modules live under the same parent file; "own" is one level up.
**Depends on**: none.

---

## One-touch tidies (each < 30 LOC, single-axis)

### T1. Inline `HUB_INIT_NAME` sentinel

`crates/domain/src/init/hub.rs:19-24` defines `const HUB_INIT_NAME: &str = "hub";` with a 5-line comment explaining it's only used at one call site (line 126: `capability_name: HUB_INIT_NAME.to_string()`). Replace with `capability_name: "hub".to_string()`. **Delta**: −8 LOC. **Done when**: `rg HUB_INIT_NAME crates/` returns nothing.

### T2. Drop trivial `registry.validate_shape_hub()?` after empty-seed write

`crates/domain/src/init/hub.rs:115-118` builds `Registry { version: 1, projects: Vec::new() }` then calls `registry.validate_shape_hub()?` with a 4-line comment admitting "Trivially passes for an empty list, but exercise the hub-mode shape check…". The validation is unreachable code. **Delta**: −5 LOC. **Done when**: `rg validate_shape_hub crates/domain/src/init/` returns nothing.

### T3. Share `sha256_hex` test helper via `tests/common/mod.rs`

`tests/tool.rs:102`, `tests/contract_tool.rs:16`, `tests/vectis_tool.rs:19` each define an identical `fn sha256_hex(path: &Path) -> String { let bytes = fs::read(path).unwrap(); format!("{:x}", Sha256::digest(&bytes)) }`. `tests/common/mod.rs` already exists for this purpose. **Delta**: −10 LOC. **Done when**: `rg "fn sha256_hex" tests/` returns exactly one match (in `tests/common/mod.rs`).

### T4. Drop forced `package_metadata` clone in `resolver.rs::stage_and_install`

After S6/S7, the field access at resolver.rs:121 (`let package_metadata = acquired.package_metadata();`) is followed by `acquired.persist_to(...)` (line 122) which consumes `acquired`. The clone is forced only because the getter clones. Pull the metadata out via destructure (`let AcquiredBytes { temp, sha256, package_metadata } = acquired;`) and pass `temp` into a free `persist_to(temp, dest)`, removing the clone. **Delta**: −1 clone, +0 LOC. **Done when**: `package_metadata` is moved exactly once on this path.

### T5. Replace `ToolForm::Object`'s field-by-field destructure with `..`-spread

`crates/tool/src/manifest.rs:161-175`: the `ToolForm::Object { name, version, source, sha256, permissions }` arm rebuilds a `Tool` with the same five fields. Either rename `ToolObject` to `Tool` directly (impossible without breaking `#[serde(from = "ToolForm")]`) or change the `From` impl body to `ToolForm::Object(obj) => obj.into()` with `impl From<ToolObject> for Tool` deriving `..obj`-style mapping. **Delta**: −10 LOC. **Done when**: the `ToolForm::Object` arm body is one expression.

### T6. Drop `Body.hub` (duplicates `capability_name == "hub"`)

`src/commands/init.rs:46-66` carries `hub: bool` on the JSON body alongside `capability_name`. The text renderer dispatches on `body.hub` (lines 69, 88); the JSON consumer can dispatch on `capability_name == "hub"` just as well. The CLI's wire compatibility note in `DECISIONS.md` allows additive removal pre-1.0. **Delta**: −5 LOC, −1 redundant boolean on the wire. **Done when**: `rg "body.hub|Body.*hub:" src/commands/init.rs` returns zero hits.

### T7. Trim study-citation prose from `omnia/code-reviewer` SKILL.md

`plugins/omnia/skills/code-reviewer/SKILL.md:22-43` opens with "Research validation: Studies show AI-generated code has **1.7× more issues than human code**…" and a six-bullet study-findings list. The skill body is 180 lines (under the 200 cap), but `docs/standards/skill-authoring.md:49` says "long-form rules, code-block examples, output templates, and edge-case enumerations belong in siblings". The justification prose burns Stage-2 token budget on every invocation. **Delta**: −22 skill-body lines. **Done when**: `wc -l plugins/omnia/skills/code-reviewer/SKILL.md` ≤ 165.

---

## Dropped during review (with reasons)

- "Promote `mod test_support` out of `lib.rs`" — would need a new module file (`crates/tool/src/test_support.rs`), forbidden by the master rule.
- "Add `clippy::module_name_repetitions` to lift `validate.rs::path_looks_windows_absolute`" — mechanical enforcement is forbidden by the master rule.
- "Split `crates/tool/src/host.rs` (457 LOC)" — under the 600-LOC tripwire cited in `docs/standards/coding-standards.md:213`; the cap is the rule.
- "Combine `has_parent_segment` in `permissions.rs:169` and `validate.rs:294`" — semantics differ (`permissions.rs` also checks `Component::ParentDir`); merging would change behaviour.
- "Add a workspace clippy lint for `pub(crate) fn foo_hex(&self) -> String { self.foo.clone() }`" — no enforceable clippy lint, < 3 occurrences.
- "Drop `tool-host-not-built` stub `WasiRunner`" — bin pulls `host` by default so the stub is dead in this workspace, but removing it changes the public crate surface for any consumer that opts out (`specify-tool` is published). Out of scope for this pass.
- "Trim 16-line block comment over `scaffold_wasm_pkg_config`" — comment-edit-only finding violates the master rule.

---

## Post-mortem

- **S1**: actual ΔLOC −57 (vs predicted −40); `rg "with_cache_env|set_or_remove_env|restore_env" crates/tool/` flipped clean to zero hits, all 57 `specify-tool` nextests + `cargo make ci` green; one regression caught by clippy and fixed in-pass — `redundant_clone` on `source_dir.clone()` in `resolver/local.rs::local_path_rejects_non_file_and_empty_file`, latent because the old closure form forced a borrow that masked the redundant clone (calibration: future closure → RAII ports should expect the same shadow lint).
- **S2**: actual ΔLOC −25 (vs predicted −25, exact); `rg "Stdio|with_stdio" crates/tool/src/host.rs` flipped clean to zero hits, host.rs landed at 432 lines (predicted ≈432, exact), all 4 `host::tests` + full `cargo make ci` green on first attempt; no regressions, no shadow lints — straight YAGNI removal of a single-axis test-only knob is a calibration-friendly baseline (clean predictions track when the deleted code is purely additive plumbing with no borrowing/closure entanglement).
