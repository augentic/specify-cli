//! Tests for `vectis scaffold` planning, writing, and the dispatcher.

use std::fs;
use std::sync::{Mutex, MutexGuard, OnceLock};

use sha2::{Digest, Sha256};
use tempfile::tempdir;

use super::templates::registry::{android, core, ios};
use super::*;

const CORE_RENDER_ONLY_SHA256: &str =
    "3db14983887828dff03d604ad449f1eb098e1008db4b83df525af6cbb64abeff";
const IOS_RENDER_ONLY_SHA256: &str =
    "74b2d27baa9ce536abe1d59d7bf75117757bb470faa73502ea28d3316c3a9699";
const ANDROID_RENDER_ONLY_SHA256: &str =
    "f35e8f62519b1d285e03d5a6f7d82b19c83072c8ff6a85ead8b7780e1e59192e";

fn versions() -> Versions {
    Versions::embedded().expect("embedded versions parse")
}

fn digest_plan(plan: &ScaffoldPlan) -> String {
    let mut hasher = Sha256::new();
    hasher.update(plan.target.as_bytes());
    hasher.update([0]);
    for file in &plan.files {
        hasher.update(file.relative_path.as_bytes());
        hasher.update([0]);
        hasher.update(file.contents.as_bytes());
        hasher.update([0]);
    }
    hasher.finalize().iter().fold(String::with_capacity(64), |mut hex, byte| {
        use std::fmt::Write as _;
        let _ = write!(hex, "{byte:02x}");
        hex
    })
}

fn env_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(())).lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[test]
fn golden_hashes_match_current_render_only_output() {
    let versions = versions();
    let core = plan_core("Counter", "com.vectis.counter", &[], &versions).unwrap();
    let ios = plan_ios("Counter", "com.vectis.counter", &[], &versions).unwrap();
    let android = plan_android("Counter", "com.vectis.counter", &[], &versions).unwrap();

    assert_eq!(digest_plan(&core), CORE_RENDER_ONLY_SHA256);
    assert_eq!(digest_plan(&ios), IOS_RENDER_ONLY_SHA256);
    assert_eq!(digest_plan(&android), ANDROID_RENDER_ONLY_SHA256);
}

#[test]
fn core_plan_preserves_template_order_and_substitutions() {
    let plan = plan_core("Counter", "com.example.counter", &[], &versions()).unwrap();
    assert_eq!(plan.files.len(), core::ENTRIES.len());
    assert_eq!(plan.files[0].relative_path, "Cargo.toml");
    assert_eq!(plan.files[8].relative_path, "shared/src/bin/codegen.rs");

    let app = plan.files.iter().find(|file| file.relative_path == "shared/src/app.rs").unwrap();
    assert!(app.contents.contains("Hello from Counter"));
    assert!(!app.contents.contains("__APP_STRUCT__"));

    let codegen =
        plan.files.iter().find(|file| file.relative_path == "shared/src/bin/codegen.rs").unwrap();
    assert!(codegen.contents.contains("com.example.counter"));
    assert!(!codegen.contents.contains("__ANDROID_PACKAGE__"));
}

#[test]
fn ios_plan_substitutes_paths_and_cap_blocks() {
    let caps = parse_caps(Some("http")).unwrap();
    let plan = plan_ios("Counter", "com.vectis.counter", &caps, &versions()).unwrap();
    assert_eq!(plan.files.len(), ios::ENTRIES.len());
    assert!(plan.files.iter().any(|file| file.relative_path == "iOS/Counter/CounterApp.swift"));

    let core_swift =
        plan.files.iter().find(|file| file.relative_path == "iOS/Counter/Core.swift").unwrap();
    assert!(core_swift.contents.contains("case .http"));
    assert!(core_swift.contents.contains("performHttpRequest"));
    assert!(!core_swift.contents.contains("<<<CAP:"));
}

#[test]
fn android_plan_skips_network_config_without_http_or_sse() {
    let plan = plan_android("Counter", "com.vectis.counter", &[], &versions()).unwrap();
    assert_eq!(plan.files.len(), android::ENTRIES.len() - 1);
    assert!(
        !plan.files.iter().any(|file| file.relative_path.ends_with("network_security_config.xml"))
    );
    assert!(plan.files.iter().any(|file| {
        file.relative_path == "Android/app/src/main/java/com/vectis/counter/CounterApplication.kt"
    }));
    assert!(
        !plan.files.iter().any(|file| file.relative_path == "Android/local.properties"),
        "host-derived local.properties is outside the WASI renderer"
    );
}

#[test]
fn android_plan_writes_network_config_when_http_enabled() {
    let caps = parse_caps(Some("http")).unwrap();
    let plan = plan_android("Counter", "com.vectis.counter", &caps, &versions()).unwrap();
    assert_eq!(plan.files.len(), android::ENTRIES.len());
    assert!(
        plan.files.iter().any(|file| file.relative_path.ends_with("network_security_config.xml"))
    );
}

#[test]
fn write_plan_refuses_existing_core_file_before_creating_dirs() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("Cargo.toml"), "pre-existing").unwrap();
    let plan = plan_core("Counter", "com.vectis.counter", &[], &versions()).unwrap();
    let err = write_plan(dir.path(), &plan).expect_err("must refuse overwrite");
    match err {
        ScaffoldError::InvalidProject { message } => {
            assert!(message.contains("refusing to overwrite existing file"));
            assert!(message.contains("Cargo.toml"));
        }
        other => panic!("unexpected error: {other:?}"),
    }
    assert!(!dir.path().join("shared/src/app.rs").exists());
    assert_eq!(fs::read_to_string(dir.path().join("Cargo.toml")).unwrap(), "pre-existing");
}

#[test]
fn write_plan_merges_existing_gitignore() {
    // `specify init` writes a root `.gitignore` in every project, so
    // the bootstrap path scaffolds into an initialised repo: the
    // template's missing lines append; operator content survives.
    let dir = tempdir().unwrap();
    fs::write(dir.path().join(".gitignore"), ".specify/cache/\n/target\n").unwrap();
    let plan = plan_core("Counter", "com.vectis.counter", &[], &versions()).unwrap();
    write_plan(dir.path(), &plan).expect("gitignore collision merges");

    let merged = fs::read_to_string(dir.path().join(".gitignore")).unwrap();
    assert!(merged.starts_with(".specify/cache/\n/target\n"), "operator content survives");
    assert!(merged.contains("# Vectis scaffold"));
    assert!(merged.contains(".DS_Store"), "template lines appended");
    assert_eq!(merged.matches("/target").count(), 1, "duplicate lines are not re-appended");
    assert!(dir.path().join("shared/src/app.rs").exists(), "rest of the plan writes normally");

    // Idempotent: a second merge pass appends nothing.
    let plan_again = plan_core("Counter", "com.vectis.counter", &[], &versions()).unwrap();
    let gitignore_template = plan_again
        .files
        .iter()
        .find(|file| file.relative_path == ".gitignore")
        .expect("core plan carries .gitignore");
    super::runtime::merge_gitignore(&dir.path().join(".gitignore"), &gitignore_template.contents)
        .expect("re-merge succeeds");
    let remerged = fs::read_to_string(dir.path().join(".gitignore")).unwrap();
    assert_eq!(merged, remerged, "second merge is a no-op");
}

#[test]
fn write_plan_refuses_existing_ios_root() {
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("iOS")).unwrap();
    let plan = plan_ios("Counter", "com.vectis.counter", &[], &versions()).unwrap();
    let err = write_plan(dir.path(), &plan).expect_err("must refuse iOS root");
    match err {
        ScaffoldError::InvalidProject { message } => {
            assert!(message.contains("refusing to overwrite existing iOS shell"));
        }
        other => panic!("unexpected error: {other:?}"),
    }
    assert!(!dir.path().join("iOS/project.yml").exists());
}

#[test]
fn write_plan_refuses_existing_android_root() {
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("Android")).unwrap();
    let plan = plan_android("Counter", "com.vectis.counter", &[], &versions()).unwrap();
    let err = write_plan(dir.path(), &plan).expect_err("must refuse Android root");
    match err {
        ScaffoldError::InvalidProject { message } => {
            assert!(message.contains("refusing to overwrite existing Android shell"));
        }
        other => panic!("unexpected error: {other:?}"),
    }
    assert!(!dir.path().join("Android/Makefile").exists());
}

#[test]
fn run_writes_under_project_dir() {
    let _guard = env_lock();
    let dir = tempdir().unwrap();
    let previous = std::env::var_os("PROJECT_DIR");
    // SAFETY: this test serializes PROJECT_DIR mutation with `env_lock`.
    let () = unsafe { std::env::set_var("PROJECT_DIR", dir.path()) };

    let command = ScaffoldCommand::Core(CoreArgs {
        common: CommonArgs {
            app_name: "Counter".into(),
            caps: None,
            version_file: None,
        },
        android_package: None,
    });
    let value = run(&command).expect("run succeeds");
    assert_eq!(value["target"], "core");
    assert!(dir.path().join("shared/src/app.rs").is_file());

    // SAFETY: this test serializes PROJECT_DIR mutation with `env_lock`.
    unsafe {
        match previous {
            Some(value) => std::env::set_var("PROJECT_DIR", value),
            None => std::env::remove_var("PROJECT_DIR"),
        }
    }
}

#[test]
fn invalid_capability_is_rejected() {
    let err = parse_caps(Some("http,bogus")).expect_err("unknown cap must fail");
    match err {
        ScaffoldError::InvalidProject { message } => {
            assert!(message.contains("\"bogus\""));
            assert!(message.contains("http"));
            assert!(message.contains("sse"));
        }
        other => panic!("unexpected error: {other:?}"),
    }
}
