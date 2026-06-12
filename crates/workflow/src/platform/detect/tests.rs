//! Unit tests for [`super::vectis_missing_platforms`].

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use jiff::Timestamp;
use specify_digest::sha256_hex;
use tempfile::tempdir;

use super::vectis_missing_platforms;
use crate::Platform;

static COUNTER: AtomicU64 = AtomicU64::new(0);

const ALL_SUPPORTED: [Platform; 3] = [Platform::Core, Platform::Ios, Platform::Android];

fn fixture_now() -> Timestamp {
    "2026-05-07T00:00:00Z".parse().expect("fixed test stamp")
}

fn env_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(())).lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn vectis_wasm() -> PathBuf {
    repo_root().join("target/vectis-wasi-tools/release/vectis.wasm")
}

fn write_project_yaml(root: &Path, platforms: &[&str]) {
    let yaml_platforms: Vec<String> = platforms.iter().map(|p| format!("  - {p}")).collect();
    let content = format!(
        "name: detect-test\nadapter: vectis\nspecify_version: '{version}'\nplatforms:\n{platforms}",
        version = env!("CARGO_PKG_VERSION"),
        platforms = yaml_platforms.join("\n"),
    );
    let specify_dir = root.join(".specify");
    fs::create_dir_all(&specify_dir).expect("mkdir .specify");
    fs::write(specify_dir.join("project.yaml"), content).expect("write project.yaml");
}

fn scaffold_core(root: &Path) {
    let dir = root.join("shared/src");
    fs::create_dir_all(&dir).expect("mkdir shared/src");
    fs::write(dir.join("app.rs"), "pub struct App;").expect("write app.rs");
}

fn scaffold_ios(root: &Path) {
    let dir = root.join("iOS/TestApp");
    fs::create_dir_all(&dir).expect("mkdir iOS/TestApp");
    fs::write(dir.join("ContentView.swift"), "struct ContentView {}").expect("write swift");
}

fn scaffold_android(root: &Path) {
    let dir = root.join("Android/app/src/main/kotlin/com/test");
    fs::create_dir_all(&dir).expect("mkdir Android");
    fs::write(dir.join("MainActivity.kt"), "class MainActivity").expect("write kt");
}

fn seed_vectis_adapter(root: &Path, wasm: &Path) {
    let adapter = root.join("adapters/targets/vectis");
    let briefs = adapter.join("briefs");
    fs::create_dir_all(&briefs).expect("mkdir briefs");
    fs::write(
        adapter.join("adapter.yaml"),
        "name: vectis\nversion: 1\naxis: target\nexecution: agent\nbriefs:\n  shape: briefs/shape.md\n  build: briefs/build.md\n  merge: briefs/merge.md\ndescription: Test vectis adapter\n",
    )
    .expect("write adapter.yaml");
    for op in ["shape", "build", "merge"] {
        fs::write(
            briefs.join(format!("{op}.md")),
            format!("---\nid: {op}\ndescription: {op} brief\n---\n"),
        )
        .expect("write brief");
    }

    let source = format!("file://{}", wasm.display());
    let digest = sha256_hex(&fs::read(wasm).expect("read wasm"));
    fs::write(
        adapter.join("tools.yaml"),
        format!(
            "tools:\n  - name: vectis\n    version: 0.4.0\n    source: \"{source}\"\n    sha256: \"{digest}\"\n    permissions:\n      read:\n        - \"$PROJECT_DIR\"\n      write: []\n"
        ),
    )
    .expect("write tools.yaml");
}

fn tools_cache_dir() -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "specify-vectis-detect-{}-{}-{n}",
        std::process::id(),
        env!("CARGO_PKG_VERSION"),
    ));
    fs::create_dir_all(&dir).expect("create tools cache");
    dir
}

struct DetectFixture {
    _cache_guard: EnvGuard,
    _home_guard: EnvGuard,
    _xdg_guard: EnvGuard,
    tmp: tempfile::TempDir,
}

impl DetectFixture {
    fn root(&self) -> &Path {
        self.tmp.path()
    }
}

struct EnvGuard {
    key: &'static str,
    previous: Option<std::ffi::OsString>,
}

impl EnvGuard {
    fn set(key: &'static str, value: &Path) -> Self {
        let previous = std::env::var_os(key);
        // SAFETY: workflow tests run serially under `cargo test -p specify-workflow`.
        #[expect(unsafe_code, reason = "test helper mutates process env for tool cache isolation")]
        unsafe {
            std::env::set_var(key, value);
        };
        Self { key, previous }
    }

    fn remove(key: &'static str) -> Self {
        let previous = std::env::var_os(key);
        // SAFETY: see [`Self::set`].
        #[expect(unsafe_code, reason = "test helper mutates process env for tool cache isolation")]
        unsafe {
            std::env::remove_var(key);
        };
        Self { key, previous }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        // SAFETY: see [`EnvGuard::set`].
        #[expect(unsafe_code, reason = "test helper restores process env on drop")]
        unsafe {
            match self.previous.take() {
                Some(value) => std::env::set_var(self.key, value),
                None => std::env::remove_var(self.key),
            }
        };
    }
}

fn detect_fixture(platforms: &[&str]) -> DetectFixture {
    let lock = env_lock();
    let wasm = vectis_wasm();
    assert!(
        wasm.is_file(),
        "vectis WASM not found at {}; run `cargo make vectis-wasm`",
        wasm.display()
    );

    let cache = tools_cache_dir();
    let cache_guard = EnvGuard::set("SPECIFY_TOOLS_CACHE", &cache);
    let home_guard = EnvGuard::remove("HOME");
    let xdg_guard = EnvGuard::remove("XDG_CACHE_HOME");
    drop(lock);

    let tmp = tempdir().expect("tempdir");
    write_project_yaml(tmp.path(), platforms);
    seed_vectis_adapter(tmp.path(), &wasm);

    DetectFixture {
        _cache_guard: cache_guard,
        _home_guard: home_guard,
        _xdg_guard: xdg_guard,
        tmp,
    }
}

#[test]
fn greenfield_reports_all_supported_missing() {
    let fixture = detect_fixture(&["core", "ios", "android"]);
    let missing =
        vectis_missing_platforms(fixture.root(), &ALL_SUPPORTED, fixture_now()).expect("detect ok");
    assert_eq!(missing, ALL_SUPPORTED.to_vec());
}

#[test]
fn partial_shells_missing_android() {
    let fixture = detect_fixture(&["core", "ios", "android"]);
    scaffold_core(fixture.root());
    scaffold_ios(fixture.root());

    let missing =
        vectis_missing_platforms(fixture.root(), &ALL_SUPPORTED, fixture_now()).expect("detect ok");
    assert_eq!(missing, vec![Platform::Android]);
}

#[test]
fn all_shells_present_returns_empty() {
    let fixture = detect_fixture(&["core", "ios", "android"]);
    scaffold_core(fixture.root());
    scaffold_ios(fixture.root());
    scaffold_android(fixture.root());

    let missing =
        vectis_missing_platforms(fixture.root(), &ALL_SUPPORTED, fixture_now()).expect("detect ok");
    assert!(missing.is_empty());
}

#[test]
fn non_vectis_skips_detect() {
    let tmp = tempdir().expect("tempdir");
    let specify_dir = tmp.path().join(".specify");
    fs::create_dir_all(&specify_dir).expect("mkdir .specify");
    fs::write(
        specify_dir.join("project.yaml"),
        format!(
            "name: omnia-app\nadapter: omnia\nspecify_version: '{version}'\nplatforms:\n  - core\n  - ios\n",
            version = env!("CARGO_PKG_VERSION"),
        ),
    )
    .expect("write project.yaml");

    let adapter = tmp.path().join("adapters/targets/omnia");
    let briefs = adapter.join("briefs");
    fs::create_dir_all(&briefs).expect("mkdir briefs");
    fs::write(
        adapter.join("adapter.yaml"),
        "name: omnia\nversion: 1\naxis: target\nexecution: agent\nbriefs:\n  shape: briefs/shape.md\n  build: briefs/build.md\n  merge: briefs/merge.md\ndescription: Test omnia adapter\n",
    )
    .expect("write adapter.yaml");
    for op in ["shape", "build", "merge"] {
        fs::write(
            briefs.join(format!("{op}.md")),
            format!("---\nid: {op}\ndescription: {op} brief\n---\n"),
        )
        .expect("write brief");
    }

    let missing =
        vectis_missing_platforms(tmp.path(), &ALL_SUPPORTED, fixture_now()).expect("non-vectis ok");
    assert!(missing.is_empty(), "non-vectis projects must not invoke detect");
}

#[test]
fn empty_declared_skips_dispatch() {
    let fixture = detect_fixture(&["core", "ios", "android"]);
    let missing =
        vectis_missing_platforms(fixture.root(), &[], fixture_now()).expect("empty declared ok");
    assert!(missing.is_empty());
}
