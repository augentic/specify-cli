#![allow(
    clippy::doc_markdown,
    reason = "The crate-level decision record mirrors RFC prose and manifest keys."
)]
#![allow(
    clippy::multiple_crate_versions,
    reason = "Wasmtime, WASI, and `wasm-pkg-client` carry unavoidable duplicate transitive crates in the workspace; surfaces in both `--features host` and `--no-default-features` builds."
)]

//! `specify-tool` owns Specify's declared WASI tool model, cache, resolver,
//! and Wasmtime-backed execution host.
//!
//! This crate is intentionally independent from `specify-capability`. The
//! binary resolves capabilities, then hands this crate project-scope and
//! capability-scope tool declarations.
//!
//! # Decisions captured up-front
//!
//! These resolve ambiguities surfaced during the readiness review so
//! implementation chunks do not re-derive them. Where a choice deviates
//! from a plain reading of the design, the rationale is recorded.
//!
//! ## Declaration sites
//!
//! Tools are declared in one or both of:
//!
//! 1. Project scope. A top-level `tools:` array in `.specify/project.yaml`.
//!    Available to every project, including hub projects that have no
//!    capability. Owned by the project author. Survives capability changes.
//! 2. Capability scope. A `tools.yaml` file as a sibling of `capability.yaml`
//!    inside the resolved capability directory. Owned by the capability author
//!    and shipped with the capability. Capabilities without a sidecar work
//!    unchanged.
//!
//! `specify tool ...` commands resolve their tool list by reading both sites
//! and merging by `name`. Project scope wins on collision so operators can
//! override capability-shipped declarations, for example to pin a different
//! version or redirect `source:` to a local copy. Conflicts emit a typed
//! `tool-name-collision` warning the first time they are observed in a session;
//! merging proceeds.
//!
//! The sidecar shape is the same `tools:` array as the project shape, so a
//! single JSON Schema (`schemas/tool.schema.json`) governs both:
//!
//! ```yaml
//! # .specify/project.yaml (project scope)
//! tools:
//!   - name: contract
//!     version: 1.0.0
//!     source: "file:///abs/path/to/contract-dev.wasm"
//!     sha256: "<hex-encoded sha256 of the component bytes>"
//!     permissions:
//!       read:  ["$PROJECT_DIR/contracts"]
//!       write: []
//!
//! # <resolved-capability-dir>/tools.yaml (capability scope)
//! tools:
//!   - "specify:contract@0.3.0"
//! ```
//!
//! `capability.yaml` is never modified by any chunk and never gains a
//! `tools:` field. The `specify-capability` crate has no knowledge of tools.
//!
//! ## Crate boundary
//!
//! - All tool types (`Tool`, `ToolPermissions`, `ToolSource`, `ToolManifest`,
//!   `ToolScope`) live in `specify-tool`. They depend only on `serde`,
//!   `semver`, `specify-error`, and, for resolution, `wasmtime`,
//!   `wasmtime-wasi`, `ureq`, `sha2`, and `dirs`. They do not depend on
//!   `specify-capability`.
//! - `specify-capability` is not modified by any chunk. The `Capability` type,
//!   its serde shape, and `schemas/capability.schema.json` are all untouched.
//! - `src/config.rs::ProjectConfig` gains a single new field:
//!   `#[serde(default)] pub tools: Vec<specify_tool::Tool>`. The `specify`
//!   binary already imports both `specify-capability` and, after chunk 5,
//!   `specify-tool`, so this introduces no cycle.
//! - `specify-tool` exposes a
//!   `load_capability_sidecar(capability_dir: &Path) -> Result<Vec<Tool>, ToolError>`
//!   helper that the binary calls after `specify-capability` resolves the
//!   capability. The resolver itself never reads `capability.yaml`.
//! - CLI dispatch lives in `src/commands/tool.rs` with a
//!   `Commands::Tool { action }` variant in `src/cli.rs`.
//!
//! ## Cache layout
//!
//! ```text
//! $SPECIFY_TOOLS_CACHE
//!   -> otherwise $XDG_CACHE_HOME/specify/tools/
//!   -> otherwise $HOME/.cache/specify/tools/
//! ```
//!
//! Within the cache root:
//!
//! ```text
//! <cache-root>/
//! └── <scope-segment>/
//!     └── <tool-name>/
//!         └── <version>/
//!             ├── module.wasm
//!             └── meta.yaml
//! ```
//!
//! - `<scope-segment>` is one of:
//!   - `project--<project-name>` for tools declared in `.specify/project.yaml`.
//!   - `capability--<capability-slug>` for tools declared in a capability
//!     sidecar `tools.yaml`.
//!     Two unrelated declarers with identical `name` fields stay isolated. The
//!     `--` separator avoids collisions with tool names that contain a hyphen.
//! - `<version>` is the literal `version:` string from the `tools[]` entry. The
//!   resolver does not parse SemVer for path computation; it only validates
//!   SemVer at structural-validation time.
//! - `module.wasm` is always the literal filename. Keeping `module.wasm` flat
//!   lets `meta.yaml` sit next to its bytes without a name-mangling rule.
//!
//! ## Sidecar metadata (`meta.yaml`)
//!
//! ```yaml
//! schema-version: 1
//! scope: <scope-segment>
//! tool-name: <name>
//! tool-version: <version>
//! source: <literal source string from manifest>
//! fetched-at: <YYYY-MM-DDThh:mm:ssZ UTC timestamp>
//! permissions-snapshot:
//!   read:  [...]
//!   write: [...]
//! sha256: <optional hex digest copied from manifest>
//! package: <optional package metadata for wasm-pkg sources>
//! oci: <optional OCI metadata for wasm-pkg sources>
//! ```
//!
//! `permissions-snapshot` is informational only in v1. Cached bytes are
//! immutable until manifest source, version, or digest changes; the cache is
//! not invalidated when permissions change because permissions are evaluated
//! per `run` against the live manifest. A sidecar whose `(scope, tool-name,
//! tool-version, source, sha256)` tuple matches the live merged manifest is a
//! cache hit. Any field mismatch forces a refetch into the same `<version>/`
//! directory with an atomic move. When `sha256` is present, the resolver
//! verifies fetched or copied bytes before installation and rejects existing
//! sidecars whose recorded digest does not match the live manifest.
//!
//! ## Permission substitution and canonicalisation
//!
//! - Substitutions apply only inside `tools[].permissions.{read,write}`
//!   entries. They do not apply to `tools[].source` and they do not apply to
//!   `--` args passed to the module.
//! - Supported variables:
//!   - `$PROJECT_DIR` is always available.
//!   - `$CAPABILITY_DIR` is only available to capability-scope tools.
//!     Project-scope declarations that reference `$CAPABILITY_DIR` are rejected
//!     at structural-validation time with `tool.capability-dir-out-of-scope`.
//! - After substitution, the path must be absolute. `..` segments are rejected
//!   before canonicalisation. The path is then canonicalised; if the canonical
//!   target is not a descendant of `PROJECT_DIR` or, for capability-scope
//!   tools, `CAPABILITY_DIR`, the request is denied even if the textual prefix
//!   matches.
//! - Project-root write preopens (`$PROJECT_DIR`) are valid for tools that
//!   must create root-level files such as `Cargo.toml`; declaration authors
//!   should still prefer the narrowest existing parent directory that satisfies
//!   the tool's contract.
//! - `permissions:` absent and `permissions: { read: [], write: [] }` are
//!   equivalent: no preopens. The structural validator accepts both.
//! - `write:` entries must not directly target Specify lifecycle state.
//!   Reject direct writes to `.specify/project.yaml`,
//!   `.specify/slices/**/.metadata.yaml`, `.specify/archive/**/.metadata.yaml`,
//!   `.specify/plan.lock`, or any directory whose intended purpose is
//!   lifecycle transition or archive movement rather than capability-owned
//!   artifacts.
//!
//! ## Argument forwarding and environment
//!
//! - `specify tool run <name> [-- <args>...]` forwards everything after `--`
//!   verbatim to the WASI module's `argv`, with `<name>` synthesised as
//!   `argv[0]`.
//! - Environment passed to the module is exactly two variables:
//!   - `PROJECT_DIR`, the canonicalised project root, always set.
//!   - `CAPABILITY_DIR`, the canonicalised resolved capability directory, set
//!     only for capability-scope tools.
//! - No host environment is inherited.
//! - Working directory of the module is the canonicalised project root.
//! - The first landing passes only explicit argv and stdio plus the two
//!   documented environment variables. Tools must not rely on inherited
//!   `PATH`, host user identity, wall-clock time, host randomness, runtime
//!   network access, or undeclared files for correctness.
//!
//! ## Exit code mapping
//!
//! | Module or runner outcome | `specify tool run` exit code |
//! | --- | --- |
//! | Module exits 0 | 0 |
//! | Module exits N, where 1 <= N <= 255 | N |
//! | Module trap or panic at runtime | 2 and a typed `runtime` error envelope |
//! | Resolver error | 2 and a typed `resolver` error envelope |
//! | Project context missing | 1 with the existing `not-initialized` envelope |
//! | Tool name not found | 2 and a typed `tool-not-declared` envelope |
//!
//! This mirrors the `0 / 1 / 2` shape `specify-contract` already
//! emits, so brief-side branching keeps working through the migration.
//!
//! ## Wasmtime configuration
//!
//! - Pin `wasmtime` and `wasmtime-wasi` to the latest stable matching pair at
//!   the time chunk 0 lands. Enable the `wasmtime-wasi` Preview 2 crate feature
//!   used by that release line (`p2` for 44.x). Use the synchronous WASI
//!   Preview 2 path (`wasmtime_wasi::add_to_linker_sync`).
//! - Use `wasmtime::component::Component` (component model), not
//!   `wasmtime::Module` (core wasm).
//! - Disable filesystem access by default in the WASI context; preopens are
//!   added per-tool from manifest permissions only.
//! - Keep execution behind the concrete `WasiRunner` boundary so manifest
//!   parsing, cache resolution, and CLI output do not depend directly on
//!   Wasmtime.
//!
//! ## Diagnostics evolution
//!
//! The first landing uses the WASI CLI command world: stdout, stderr, and exit
//! code are the diagnostic channel. That is acceptable for the initial contract
//! validator migration, but it is not the final validator ABI. When a helper
//! needs machine-readable findings that skills must parse, add a custom WIT
//! world with typed diagnostic exports and keep a thin command-world wrapper
//! only for manual invocation.
//!
//! ## Cache concurrency
//!
//! No file locks in v1. Two concurrent `specify tool run` invocations on a cold
//! cache may both fetch and stage; the atomic rename in the resolver makes the
//! steady state deterministic. A per-tool flock is deferred until it is needed.
//!
//! ## `specify tool gc` scope
//!
//! `gc` deletes any `<cache-root>/<scope-segment>/<tool-name>/<version>/` whose
//! `(scope, name, version, source)` tuple is not referenced by the merged tool
//! list of the current project (`project.yaml` plus the resolved-capability
//! sidecar, when present). It does not scan other projects on the host.

pub mod cache;
pub mod error;
#[cfg(feature = "host")]
pub mod host;
#[cfg(not(feature = "host"))]
#[path = "host_stub.rs"]
pub mod host;
pub mod load;
pub mod manifest;
pub mod package;
pub mod permissions;
pub mod resolver;
pub mod validate;

pub use error::ToolError;
pub use manifest::{Tool, ToolManifest, ToolPermissions, ToolScope, ToolSource};

#[cfg(test)]
pub(crate) mod test_support {
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Mutex, MutexGuard, OnceLock};
    use std::time::{SystemTime, UNIX_EPOCH};
    use std::{env, fs};

    use chrono::{DateTime, Utc};

    use crate::cache;
    use crate::manifest::{Tool, ToolPermissions, ToolScope, ToolSource};

    static SCRATCH_COUNTER: AtomicU64 = AtomicU64::new(0);

    pub(crate) fn fixed_now() -> DateTime<Utc> {
        "2026-05-07T00:00:00Z".parse().expect("fixed test stamp")
    }

    pub(crate) fn project_scope() -> ToolScope {
        ToolScope::Project {
            project_name: "demo".to_string(),
        }
    }

    pub(crate) fn capability_scope(root: &Path) -> ToolScope {
        ToolScope::Capability {
            capability_slug: "contracts".to_string(),
            capability_dir: root.to_path_buf(),
        }
    }

    pub(crate) fn tool(source: ToolSource, sha256: Option<String>) -> Tool {
        Tool {
            name: "contract".to_string(),
            version: "1.0.0".to_string(),
            source,
            sha256,
            permissions: ToolPermissions::default(),
        }
    }

    pub(crate) fn named_tool(name: &str, source: ToolSource, sha256: Option<String>) -> Tool {
        Tool {
            name: name.to_string(),
            ..tool(source, sha256)
        }
    }

    pub(crate) fn write_source(root: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        let path = root.join(name);
        std::fs::write(&path, bytes).expect("write source");
        path
    }

    pub(crate) fn cached_bytes(scope: &ToolScope, tool: &Tool) -> Vec<u8> {
        std::fs::read(cache::module_path(scope, &tool.name, &tool.version).expect("module path"))
            .expect("read cached module")
    }

    /// Lock guarding process-wide environment mutations in tests.
    pub(crate) fn env_lock() -> MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Create a unique temporary directory for tests.
    pub(crate) fn scratch_dir(label: &str) -> PathBuf {
        let n = SCRATCH_COUNTER.fetch_add(1, Ordering::Relaxed);
        let nanos =
            SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |duration| duration.as_nanos());
        let dir = env::temp_dir()
            .join(format!("specify-tool-{label}-{}-{nanos}-{n}", std::process::id()));
        fs::create_dir_all(&dir).expect("create scratch dir");
        dir
    }

    /// Run a closure with cache-related environment variables set.
    pub(crate) fn with_cache_env<T>(
        specify_cache: Option<&Path>, xdg_cache: Option<&Path>, home: Option<&Path>,
        f: impl FnOnce() -> T,
    ) -> T {
        let _guard = env_lock();
        let previous_specify = env::var_os("SPECIFY_TOOLS_CACHE");
        let previous_xdg = env::var_os("XDG_CACHE_HOME");
        let previous_home = env::var_os("HOME");

        set_or_remove_env("SPECIFY_TOOLS_CACHE", specify_cache);
        set_or_remove_env("XDG_CACHE_HOME", xdg_cache);
        set_or_remove_env("HOME", home);

        let result = f();

        restore_env("SPECIFY_TOOLS_CACHE", previous_specify);
        restore_env("XDG_CACHE_HOME", previous_xdg);
        restore_env("HOME", previous_home);

        result
    }

    fn set_or_remove_env(key: &str, value: Option<&Path>) {
        // SAFETY: every test that mutates these process-wide environment
        // variables goes through `env_lock`, preventing concurrent readers from
        // observing partial setup or teardown.
        unsafe {
            match value {
                Some(value) => env::set_var(key, value),
                None => env::remove_var(key),
            }
        }
    }

    fn restore_env(key: &str, value: Option<std::ffi::OsString>) {
        // SAFETY: protected by `env_lock`; see `set_or_remove_env`.
        unsafe {
            match value {
                Some(value) => env::set_var(key, value),
                None => env::remove_var(key),
            }
        }
    }
}
