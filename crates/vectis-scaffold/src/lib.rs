//! Render-only implementation for the `vectis-scaffold` WASI command tool.
//!
//! The crate accepts only explicit CLI inputs and the `PROJECT_DIR` declared by
//! the RFC-15 host. It renders embedded templates, plans every target file, then
//! refuses all overwrites before creating directories or writing bytes.

mod error;
mod templates;
mod versions;

use std::fs;
use std::path::{Path, PathBuf};

use clap::{Args as ClapArgs, Parser, Subcommand};
use serde::Serialize;
use templates::{Params, android, core, ios, render, substitute_path, substitute_path_with};

pub use error::ScaffoldError;
pub use templates::Capability;
pub use versions::Versions;

/// JSON contract version emitted on structured responses.
pub const JSON_SCHEMA_VERSION: u64 = 2;

/// Arguments accepted by `vectis-scaffold`.
#[derive(Parser, Debug, Clone, PartialEq, Eq)]
#[command(
    name = "vectis-scaffold",
    version,
    about = "Render Vectis Crux scaffolds.",
    long_about = "Render Vectis Crux scaffolds using the RFC-16 WASI command surface.\n\
                  \nThe command writes under PROJECT_DIR, accepts embedded version pins or an \
                  explicit complete --version-file override, and performs no host SDK discovery \
                  or build-tool execution."
)]
pub struct Args {
    /// Scaffold target to render.
    #[command(subcommand)]
    pub command: ScaffoldCommand,
}

/// Scaffold targets preserved for the WASI command surface.
#[derive(Subcommand, Debug, Clone, PartialEq, Eq)]
pub enum ScaffoldCommand {
    /// Render the shared Rust Crux core crate.
    Core(CoreArgs),
    /// Render the `SwiftUI` iOS shell.
    Ios(IosArgs),
    /// Render the Jetpack Compose Android shell.
    Android(AndroidArgs),
}

impl ScaffoldCommand {
    /// Return the stable CLI spelling for this scaffold target.
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Core(_) => "core",
            Self::Ios(_) => "ios",
            Self::Android(_) => "android",
        }
    }

    /// Return the app name supplied to this scaffold target.
    #[must_use]
    pub fn app_name(&self) -> &str {
        match self {
            Self::Core(args) => &args.common.app_name,
            Self::Ios(args) => &args.common.app_name,
            Self::Android(args) => &args.common.app_name,
        }
    }

    /// Return common arguments for this command.
    #[must_use]
    pub const fn common(&self) -> &CommonArgs {
        match self {
            Self::Core(args) => &args.common,
            Self::Ios(args) => &args.common,
            Self::Android(args) => &args.common,
        }
    }
}

/// Arguments for `vectis-scaffold core`.
#[derive(ClapArgs, Debug, Clone, PartialEq, Eq)]
pub struct CoreArgs {
    /// Common app, capability, and version arguments.
    #[command(flatten)]
    pub common: CommonArgs,

    /// Android package name used when rendering Android-facing core bindings.
    #[arg(long)]
    pub android_package: Option<String>,
}

/// Arguments for `vectis-scaffold ios`.
#[derive(ClapArgs, Debug, Clone, PartialEq, Eq)]
pub struct IosArgs {
    /// Common app, capability, and version arguments.
    #[command(flatten)]
    pub common: CommonArgs,
}

/// Arguments for `vectis-scaffold android`.
#[derive(ClapArgs, Debug, Clone, PartialEq, Eq)]
pub struct AndroidArgs {
    /// Common app, capability, and version arguments.
    #[command(flatten)]
    pub common: CommonArgs,

    /// Android application package name.
    #[arg(long)]
    pub android_package: Option<String>,
}

/// Arguments shared by all scaffold targets.
#[derive(ClapArgs, Debug, Clone, PartialEq, Eq)]
pub struct CommonArgs {
    /// App struct/name to scaffold, for example `Counter` or `TodoApp`.
    pub app_name: String,

    /// Comma-separated capabilities, for example `http,kv,time`.
    #[arg(long)]
    pub caps: Option<String>,

    /// Complete TOML file overriding the embedded version defaults.
    #[arg(long)]
    pub version_file: Option<PathBuf>,
}

/// A rendered file ready to write under `PROJECT_DIR`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedFile {
    /// Relative target path under `PROJECT_DIR`.
    pub relative_path: String,
    /// Rendered file bytes as UTF-8 text.
    pub contents: String,
}

/// A complete scaffold plan for one target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScaffoldPlan {
    /// Scaffold target name.
    pub target: &'static str,
    /// App name supplied by the caller.
    pub app_name: String,
    /// Android package used for placeholders.
    pub android_package: String,
    /// Selected capability tags in stable input order.
    pub capabilities: Vec<String>,
    /// Files to write in template declaration order.
    pub files: Vec<PlannedFile>,
}

impl ScaffoldPlan {
    fn file_paths(&self) -> Vec<String> {
        self.files.iter().map(|file| file.relative_path.clone()).collect()
    }

    fn to_json(&self, project_dir: &Path) -> serde_json::Value {
        serde_json::json!({
            "target": self.target,
            "app-name": self.app_name,
            "project-dir": project_dir.display().to_string(),
            "android-package": self.android_package,
            "capabilities": self.capabilities,
            "files": self.file_paths(),
        })
    }
}

/// Execute a parsed scaffold invocation.
///
/// # Errors
///
/// Returns [`ScaffoldError`] for invalid inputs, version-file issues, missing
/// `PROJECT_DIR`, or write failures.
pub fn run(args: &Args) -> Result<serde_json::Value, ScaffoldError> {
    let project_dir = project_dir_from_env()?;
    let versions = Versions::resolve(args.command.common().version_file.as_deref())?;
    let plan = plan_command(&args.command, &versions)?;
    write_plan(&project_dir, &plan)?;
    Ok(plan.to_json(&project_dir))
}

/// Plan a scaffold command without touching the filesystem.
///
/// # Errors
///
/// Returns [`ScaffoldError`] when arguments are invalid.
pub fn plan_command(
    command: &ScaffoldCommand, versions: &Versions,
) -> Result<ScaffoldPlan, ScaffoldError> {
    match command {
        ScaffoldCommand::Core(args) => {
            let caps = parse_caps(args.common.caps.as_deref())?;
            let android_package = args
                .android_package
                .clone()
                .unwrap_or_else(|| default_android_package(&args.common.app_name));
            plan_core(&args.common.app_name, &android_package, &caps, versions)
        }
        ScaffoldCommand::Ios(args) => {
            let caps = parse_caps(args.common.caps.as_deref())?;
            let android_package = default_android_package(&args.common.app_name);
            plan_ios(&args.common.app_name, &android_package, &caps, versions)
        }
        ScaffoldCommand::Android(args) => {
            let caps = parse_caps(args.common.caps.as_deref())?;
            let android_package = args
                .android_package
                .clone()
                .unwrap_or_else(|| default_android_package(&args.common.app_name));
            plan_android(&args.common.app_name, &android_package, &caps, versions)
        }
    }
}

/// Render and plan the core scaffold.
///
/// # Errors
///
/// Returns [`ScaffoldError`] when the app name is invalid.
pub fn plan_core(
    app_name: &str, android_package: &str, caps: &[Capability], versions: &Versions,
) -> Result<ScaffoldPlan, ScaffoldError> {
    validate_app_name(app_name)?;
    let params = build_params(app_name, android_package, versions);
    let files = core::TEMPLATES
        .iter()
        .map(|entry| PlannedFile {
            relative_path: entry.target.to_string(),
            contents: render(entry.contents, &params, caps),
        })
        .collect();
    Ok(scaffold_plan("core", app_name, android_package, caps, files))
}

/// Render and plan the iOS shell scaffold.
///
/// # Errors
///
/// Returns [`ScaffoldError`] when the app name is invalid.
pub fn plan_ios(
    app_name: &str, android_package: &str, caps: &[Capability], versions: &Versions,
) -> Result<ScaffoldPlan, ScaffoldError> {
    validate_app_name(app_name)?;
    let params = build_params(app_name, android_package, versions);
    let files = ios::TEMPLATES
        .iter()
        .map(|entry| PlannedFile {
            relative_path: substitute_path(entry.target, &params),
            contents: render(entry.contents, &params, caps),
        })
        .collect();
    Ok(scaffold_plan("ios", app_name, android_package, caps, files))
}

/// Render and plan the Android shell scaffold.
///
/// # Errors
///
/// Returns [`ScaffoldError`] when the app name is invalid.
pub fn plan_android(
    app_name: &str, android_package: &str, caps: &[Capability], versions: &Versions,
) -> Result<ScaffoldPlan, ScaffoldError> {
    validate_app_name(app_name)?;
    let params = build_params(app_name, android_package, versions);
    let android_package_path = android_package_to_path(android_package);
    let files = android::TEMPLATES
        .iter()
        .filter(|entry| entry.include_when.should_include(caps))
        .map(|entry| PlannedFile {
            relative_path: substitute_path_with(entry.target, &params, Some(&android_package_path)),
            contents: render(entry.contents, &params, caps),
        })
        .collect();
    Ok(scaffold_plan("android", app_name, android_package, caps, files))
}

/// Write a complete plan under `project_dir` after checking every collision.
///
/// # Errors
///
/// Returns [`ScaffoldError`] if any target already exists or a write fails.
pub fn write_plan(project_dir: &Path, plan: &ScaffoldPlan) -> Result<(), ScaffoldError> {
    match plan.target {
        "ios" => refuse_existing_root(project_dir, "iOS", "iOS")?,
        "android" => refuse_existing_root(project_dir, "Android", "Android")?,
        _ => {}
    }

    for file in &plan.files {
        let target = project_dir.join(&file.relative_path);
        if target.exists() {
            return Err(ScaffoldError::InvalidProject {
                message: format!(
                    "refusing to overwrite existing file at {} (run `vectis-scaffold` against an empty target)",
                    target.display()
                ),
            });
        }
    }

    if !project_dir.exists() {
        fs::create_dir_all(project_dir)?;
    }
    for file in &plan.files {
        let target = project_dir.join(&file.relative_path);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(target, &file.contents)?;
    }
    Ok(())
}

/// Compute the default Android package: `com.vectis.<lower app name>`.
#[must_use]
pub fn default_android_package(app_name: &str) -> String {
    format!("com.vectis.{}", app_name.to_lowercase())
}

/// Validate `app_name` as `PascalCase` ASCII.
///
/// # Errors
///
/// Returns [`ScaffoldError`] when the app name cannot be used as a generated
/// Rust/Swift/Kotlin identifier segment.
pub fn validate_app_name(app_name: &str) -> Result<(), ScaffoldError> {
    let mut chars = app_name.chars();
    let first = chars.next().ok_or_else(|| ScaffoldError::InvalidProject {
        message: "app name must not be empty".into(),
    })?;
    if !first.is_ascii_uppercase() {
        return Err(ScaffoldError::InvalidProject {
            message: format!(
                "app name {app_name:?} must start with an ASCII uppercase letter (PascalCase, e.g. \"Counter\")"
            ),
        });
    }
    for c in chars {
        if !c.is_ascii_alphanumeric() {
            return Err(ScaffoldError::InvalidProject {
                message: format!(
                    "app name {app_name:?} must contain only ASCII alphanumeric characters (PascalCase)"
                ),
            });
        }
    }
    Ok(())
}

/// Parse the `--caps` flag into the canonical capability set.
///
/// # Errors
///
/// Returns [`ScaffoldError`] when an unknown capability tag is present.
pub fn parse_caps(raw: Option<&str>) -> Result<Vec<Capability>, ScaffoldError> {
    let mut out: Vec<Capability> = Vec::new();
    let Some(raw) = raw else { return Ok(out) };
    for tag in raw.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        let cap = Capability::from_tag(tag).ok_or_else(|| ScaffoldError::InvalidProject {
            message: format!(
                "unknown capability: {tag:?} (expected one of: http, kv, time, platform, sse)"
            ),
        })?;
        if !out.contains(&cap) {
            out.push(cap);
        }
    }
    Ok(out)
}

/// Render a `(success | error)` result as the v2 JSON envelope.
#[must_use]
pub fn render_envelope_json(outcome: Result<serde_json::Value, ScaffoldError>) -> (String, u8) {
    match outcome {
        Ok(value) => (envelope_json(value), 0),
        Err(err) => {
            let exit_code = err.exit_code();
            let serde_json::Value::Object(mut payload) = err.to_json() else {
                unreachable!("ScaffoldError::to_json always returns an object")
            };
            payload.entry("exit-code".to_string()).or_insert(serde_json::Value::from(exit_code));
            (envelope_json(serde_json::Value::Object(payload)), exit_code)
        }
    }
}

fn envelope_json(payload: serde_json::Value) -> String {
    #[derive(Serialize)]
    struct Envelope {
        #[serde(rename = "schema-version")]
        schema_version: u64,
        #[serde(flatten)]
        payload: serde_json::Value,
    }

    serde_json::to_string_pretty(&Envelope {
        schema_version: JSON_SCHEMA_VERSION,
        payload,
    })
    .expect("JSON serialise")
}

fn scaffold_plan(
    target: &'static str, app_name: &str, android_package: &str, caps: &[Capability],
    files: Vec<PlannedFile>,
) -> ScaffoldPlan {
    ScaffoldPlan {
        target,
        app_name: app_name.to_string(),
        android_package: android_package.to_string(),
        capabilities: caps.iter().map(|c| c.marker_tag().to_string()).collect(),
        files,
    }
}

fn build_params(app_name: &str, android_package: &str, versions: &Versions) -> Params {
    Params {
        app_name: app_name.to_string(),
        app_struct: app_name.to_string(),
        app_name_lower: app_name.to_lowercase(),
        android_package: android_package.to_string(),
        crux_core_version: versions.crux.crux_core.clone(),
        crux_http_version: versions.crux.crux_http.clone(),
        crux_kv_version: versions.crux.crux_kv.clone(),
        crux_time_version: versions.crux.crux_time.clone(),
        crux_platform_version: versions.crux.crux_platform.clone(),
        facet_version: versions.crux.facet.clone(),
        serde_version: versions.crux.serde.clone(),
        uniffi_version: versions.crux.uniffi.clone(),
        agp_version: versions.android.agp.clone(),
        kotlin_version: versions.android.kotlin.clone(),
        compose_bom_version: versions.android.compose_bom.clone(),
        ktor_version: versions.android.ktor.clone(),
        koin_version: versions.android.koin.clone(),
        android_ndk_version: versions
            .android
            .ndk
            .clone()
            .unwrap_or_else(|| "__ANDROID_NDK_VERSION__".to_string()),
    }
}

fn refuse_existing_root(
    project_dir: &Path, root: &str, display_name: &str,
) -> Result<(), ScaffoldError> {
    let shell_root = project_dir.join(root);
    if shell_root.exists() {
        return Err(ScaffoldError::InvalidProject {
            message: format!(
                "refusing to overwrite existing {display_name} shell at {} (delete it first or use the host add-shell workflow)",
                shell_root.display()
            ),
        });
    }
    Ok(())
}

fn android_package_to_path(pkg: &str) -> String {
    pkg.replace('.', "/")
}

fn project_dir_from_env() -> Result<PathBuf, ScaffoldError> {
    std::env::var_os("PROJECT_DIR").map(PathBuf::from).ok_or_else(|| {
        ScaffoldError::InvalidProject {
            message: "PROJECT_DIR is not set; run through `specify tool run` with a project scope"
                .into(),
        }
    })
}

#[cfg(test)]
mod tests {
    use std::sync::{Mutex, MutexGuard, OnceLock};

    use sha2::{Digest, Sha256};
    use tempfile::tempdir;

    use super::*;

    const CORE_RENDER_ONLY_SHA256: &str =
        "675f182ec847e4f4238cf5619b9635b323366df617f9b70619edeacc4033bdc7";
    const IOS_RENDER_ONLY_SHA256: &str =
        "74b2d27baa9ce536abe1d59d7bf75117757bb470faa73502ea28d3316c3a9699";
    const ANDROID_RENDER_ONLY_SHA256: &str =
        "3557768fcb9aa9e65bdacee242976573dd1d202e4471afd607c721b676520a77";

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
        format!("{:x}", hasher.finalize())
    }

    fn env_lock() -> MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
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
        assert_eq!(plan.files.len(), core::TEMPLATES.len());
        assert_eq!(plan.files[0].relative_path, "Cargo.toml");
        assert_eq!(plan.files[8].relative_path, "shared/src/bin/codegen.rs");

        let app = plan.files.iter().find(|file| file.relative_path == "shared/src/app.rs").unwrap();
        assert!(app.contents.contains("Hello from Counter"));
        assert!(!app.contents.contains("__APP_STRUCT__"));

        let codegen = plan
            .files
            .iter()
            .find(|file| file.relative_path == "shared/src/bin/codegen.rs")
            .unwrap();
        assert!(codegen.contents.contains("com.example.counter"));
        assert!(!codegen.contents.contains("__ANDROID_PACKAGE__"));
    }

    #[test]
    fn ios_plan_substitutes_paths_and_cap_blocks() {
        let caps = parse_caps(Some("http")).unwrap();
        let plan = plan_ios("Counter", "com.vectis.counter", &caps, &versions()).unwrap();
        assert_eq!(plan.files.len(), ios::TEMPLATES.len());
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
        assert_eq!(plan.files.len(), android::TEMPLATES.len() - 1);
        assert!(
            !plan
                .files
                .iter()
                .any(|file| file.relative_path.ends_with("network_security_config.xml"))
        );
        assert!(plan.files.iter().any(|file| {
            file.relative_path
                == "Android/app/src/main/java/com/vectis/counter/CounterApplication.kt"
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
        assert_eq!(plan.files.len(), android::TEMPLATES.len());
        assert!(
            plan.files
                .iter()
                .any(|file| file.relative_path.ends_with("network_security_config.xml"))
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
        #[allow(clippy::semicolon_if_nothing_returned)]
        let () = unsafe { std::env::set_var("PROJECT_DIR", dir.path()) };

        let args = Args {
            command: ScaffoldCommand::Core(CoreArgs {
                common: CommonArgs {
                    app_name: "Counter".into(),
                    caps: None,
                    version_file: None,
                },
                android_package: None,
            }),
        };
        let value = run(&args).expect("run succeeds");
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
}
