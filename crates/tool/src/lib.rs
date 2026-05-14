#![allow(
    clippy::doc_markdown,
    reason = "The crate-level decision record mirrors RFC prose and manifest keys."
)]

//! Specify's declared WASI tool model, cache, resolver, and
//! Wasmtime-backed execution host. See `DECISIONS.md`
//! §"Tool architecture" for the canonical contract.

pub mod cache;
pub mod error;
pub mod host;
pub mod load;
pub mod manifest;
pub mod package;
pub mod permissions;
pub mod resolver;
pub mod validate;

pub use error::ToolError;
pub use manifest::{Tool, ToolManifest, ToolPermissions, ToolScope, ToolScopeKind, ToolSource};

#[cfg(test)]
#[expect(unsafe_code, reason = "test helpers mutate process-wide env vars under env_lock")]
pub(crate) mod test_support {
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Mutex, MutexGuard, OnceLock};
    use std::time::{SystemTime, UNIX_EPOCH};
    use std::{env, fs};

    use jiff::Timestamp;

    use crate::cache;
    use crate::manifest::{Tool, ToolPermissions, ToolScope, ToolSource};

    static SCRATCH_COUNTER: AtomicU64 = AtomicU64::new(0);

    pub fn fixed_now() -> Timestamp {
        "2026-05-07T00:00:00Z".parse().expect("fixed test stamp")
    }

    pub fn project_scope() -> ToolScope {
        ToolScope::Project {
            project_name: "demo".to_string(),
        }
    }

    pub fn capability_scope(root: &Path) -> ToolScope {
        ToolScope::Capability {
            capability_slug: "contracts".to_string(),
            capability_dir: root.to_path_buf(),
        }
    }

    pub fn tool(source: ToolSource, sha256: Option<String>) -> Tool {
        Tool {
            name: "contract".to_string(),
            version: "1.0.0".to_string(),
            source,
            sha256,
            permissions: ToolPermissions::default(),
        }
    }

    pub fn named_tool(name: &str, source: ToolSource, sha256: Option<String>) -> Tool {
        Tool {
            name: name.to_string(),
            ..tool(source, sha256)
        }
    }

    pub fn write_source(root: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        let path = root.join(name);
        fs::write(&path, bytes).expect("write source");
        path
    }

    pub fn cached_bytes(scope: &ToolScope, tool: &Tool) -> Vec<u8> {
        fs::read(cache::module_path(scope, &tool.name, &tool.version).expect("module path"))
            .expect("read cached module")
    }

    /// Lock guarding process-wide environment mutations in tests.
    pub fn env_lock() -> MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Create a unique temporary directory for tests.
    pub fn scratch_dir(label: &str) -> PathBuf {
        let n = SCRATCH_COUNTER.fetch_add(1, Ordering::Relaxed);
        let nanos =
            SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |duration| duration.as_nanos());
        let dir = env::temp_dir()
            .join(format!("specify-tool-{label}-{}-{nanos}-{n}", std::process::id()));
        fs::create_dir_all(&dir).expect("create scratch dir");
        dir
    }

    /// Run a closure with cache-related environment variables set.
    pub fn with_cache_env<T>(
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
