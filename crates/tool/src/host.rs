//! Wasmtime-backed WASI Preview 2 runner boundary.
//!
//! `Stdio` and `RunContext` are always compiled; only `WasiRunner` and
//! its wasmtime-dependent helpers are gated behind the `host` Cargo
//! feature. Builds without `host` get a stub `WasiRunner` whose `run`
//! returns the `tool-host-not-built` diagnostic.

#[cfg(feature = "host")]
use std::collections::BTreeMap;
#[cfg(feature = "host")]
use std::path::Path;
use std::path::PathBuf;

#[cfg(feature = "host")]
use wasmtime::component::{Component, Linker, ResourceTable};
#[cfg(feature = "host")]
use wasmtime::{Config, Engine, Store};
#[cfg(feature = "host")]
use wasmtime_wasi::p2::bindings::sync::Command;
#[cfg(feature = "host")]
use wasmtime_wasi::{DirPerms, FilePerms, I32Exit, WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};

use crate::error::ToolError;
#[cfg(feature = "host")]
use crate::manifest::ToolScope;
#[cfg(feature = "host")]
use crate::permissions::{canonicalise_under, deny_lifecycle_write, substitute};
use crate::resolver::ResolvedTool;

/// Stdio configuration for a tool run.
#[derive(Debug, Copy, Clone, Default, PartialEq, Eq)]
pub enum Stdio {
    /// Explicitly inherit stdin, stdout, and stderr from the host process.
    #[default]
    Inherit,
    /// Keep Wasmtime's closed stdin and sink stdout/stderr defaults.
    Null,
}

/// Host-side context for running a resolved tool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunContext {
    /// Project root used for `$PROJECT_DIR` and permission-root checks.
    pub project_dir: PathBuf,
    /// Canonical or canonicalisable capability root for capability-scope tools.
    pub capability_dir: Option<PathBuf>,
    /// Arguments forwarded after `argv[0]`, which is always the tool name.
    pub args: Vec<String>,
    /// Stdio handling for the WASI context.
    pub stdio: Stdio,
}

impl RunContext {
    /// Construct a run context with inherited stdio.
    #[must_use]
    pub fn new(project_dir: impl Into<PathBuf>, args: Vec<String>) -> Self {
        Self {
            project_dir: project_dir.into(),
            capability_dir: None,
            args,
            stdio: Stdio::Inherit,
        }
    }

    /// Attach a capability root for capability-scope tools.
    #[must_use]
    pub fn with_capability_dir(mut self, capability_dir: impl Into<PathBuf>) -> Self {
        self.capability_dir = Some(capability_dir.into());
        self
    }

    /// Override stdio handling.
    #[must_use]
    pub const fn with_stdio(mut self, stdio: Stdio) -> Self {
        self.stdio = stdio;
        self
    }
}

/// Wasmtime-backed synchronous WASI Preview 2 runner.
///
/// When the crate is built without the `host` feature this becomes a stub
/// whose `run` returns the `tool-host-not-built` diagnostic; the public
/// surface is preserved so plan-time callers compile against either build.
#[cfg(feature = "host")]
#[expect(missing_debug_implementations, reason = "wraps non-Debug wasmtime::Engine")]
pub struct WasiRunner {
    engine: Engine,
}

#[cfg(feature = "host")]
impl WasiRunner {
    /// Construct a reusable Wasmtime engine for WASI Preview 2 components.
    ///
    /// # Errors
    ///
    /// Returns a runtime error when the engine configuration cannot be built.
    pub fn new() -> Result<Self, ToolError> {
        let mut config = Config::new();
        config.wasm_component_model(true);
        let engine = Engine::new(&config).map_err(|err| {
            ToolError::runtime(format!("failed to create Wasmtime engine: {err}"))
        })?;
        Ok(Self { engine })
    }

    /// Run a resolved WASI tool and return the guest's process-style exit code.
    ///
    /// # Errors
    ///
    /// Returns permission errors before instantiation, or runtime errors when
    /// Wasmtime cannot compile, link, instantiate, or execute the component.
    pub fn run(&self, resolved: &ResolvedTool, ctx: &RunContext) -> Result<i32, ToolError> {
        let project_dir = canonical_project_dir(&ctx.project_dir)?;
        let capability_dir =
            canonical_capability_dir(&resolved.scope, ctx.capability_dir.as_deref())?;
        let preopens = prepare_preopens(resolved, &project_dir, capability_dir.as_deref())?;
        let wasi =
            build_wasi_ctx(resolved, ctx, &project_dir, capability_dir.as_deref(), &preopens)?;

        let component = Component::from_file(&self.engine, &resolved.bytes_path)
            .map_err(|err| ToolError::runtime(format!("failed to compile component: {err}")))?;
        let mut linker = Linker::<WasiState>::new(&self.engine);
        let mut link_options = wasmtime_wasi::p2::bindings::sync::LinkOptions::default();
        link_options.cli_exit_with_code(true);
        wasmtime_wasi::p2::add_to_linker_with_options_sync(&mut linker, &link_options)
            .map_err(|err| ToolError::runtime(format!("failed to link WASI Preview 2: {err}")))?;
        let mut store = Store::new(
            &self.engine,
            WasiState {
                ctx: wasi,
                table: ResourceTable::new(),
            },
        );
        let command = Command::instantiate(&mut store, &component, &linker)
            .map_err(|err| ToolError::runtime(format!("failed to instantiate command: {err}")))?;

        match command.wasi_cli_run().call_run(&mut store) {
            Ok(Ok(())) => Ok(0),
            Ok(Err(())) => Ok(1),
            Err(err) => map_guest_error(&err),
        }
    }
}

#[cfg(feature = "host")]
struct WasiState {
    ctx: WasiCtx,
    table: ResourceTable,
}

#[cfg(feature = "host")]
impl WasiView for WasiState {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.ctx,
            table: &mut self.table,
        }
    }
}

#[cfg(feature = "host")]
#[derive(Debug, Clone, PartialEq, Eq)]
struct Preopen {
    host_path: PathBuf,
    guest_path: String,
    writable: bool,
}

#[cfg(feature = "host")]
fn canonical_project_dir(project_dir: &Path) -> Result<PathBuf, ToolError> {
    project_dir.canonicalize().map_err(|err| {
        ToolError::permission_denied(
            project_dir,
            format!("PROJECT_DIR must exist and be canonicalisable: {err}"),
        )
    })
}

#[cfg(feature = "host")]
fn canonical_capability_dir(
    scope: &ToolScope, ctx_capability_dir: Option<&Path>,
) -> Result<Option<PathBuf>, ToolError> {
    match scope {
        ToolScope::Project { .. } => Ok(None),
        ToolScope::Capability { capability_dir, .. } => {
            let path = ctx_capability_dir.unwrap_or(capability_dir);
            let canonical = path.canonicalize().map_err(|err| {
                ToolError::permission_denied(
                    path,
                    format!("CAPABILITY_DIR must exist and be canonicalisable: {err}"),
                )
            })?;
            Ok(Some(canonical))
        }
    }
}

#[cfg(feature = "host")]
fn prepare_preopens(
    resolved: &ResolvedTool, project_dir: &Path, capability_dir: Option<&Path>,
) -> Result<Vec<Preopen>, ToolError> {
    let mut roots = vec![project_dir];
    if let Some(capability_dir) = capability_dir {
        roots.push(capability_dir);
    }

    let mut permissions = BTreeMap::<PathBuf, bool>::new();
    for template in &resolved.tool.permissions.read {
        let expanded = substitute(template, project_dir, capability_dir)?;
        let canonical = canonicalise_under(Path::new(&expanded), &roots)?;
        permissions.entry(canonical).or_insert(false);
    }
    for template in &resolved.tool.permissions.write {
        let expanded = substitute(template, project_dir, capability_dir)?;
        let canonical = canonicalise_under(Path::new(&expanded), &roots)?;
        deny_lifecycle_write(&canonical, project_dir)?;
        permissions.insert(canonical, true);
    }

    permissions
        .into_iter()
        .map(|(host_path, writable)| {
            let guest_path = guest_path(&host_path)?;
            Ok(Preopen {
                host_path,
                guest_path,
                writable,
            })
        })
        .collect()
}

#[cfg(feature = "host")]
fn build_wasi_ctx(
    resolved: &ResolvedTool, ctx: &RunContext, project_dir: &Path, capability_dir: Option<&Path>,
    preopens: &[Preopen],
) -> Result<WasiCtx, ToolError> {
    let mut builder = WasiCtxBuilder::new();
    builder
        .allow_blocking_current_thread(true)
        .allow_tcp(false)
        .allow_udp(false)
        .allow_ip_name_lookup(false);

    match ctx.stdio {
        Stdio::Inherit => {
            builder.inherit_stdio();
        }
        Stdio::Null => {}
    }

    let mut argv = Vec::with_capacity(ctx.args.len() + 1);
    argv.push(resolved.tool.name.clone());
    argv.extend(ctx.args.iter().cloned());
    builder.args(&argv);

    builder.env("PROJECT_DIR", path_to_env(project_dir, "PROJECT_DIR")?);
    if let Some(capability_dir) = capability_dir {
        builder.env("CAPABILITY_DIR", path_to_env(capability_dir, "CAPABILITY_DIR")?);
    }

    for preopen in preopens {
        let (dir_perms, file_perms) = if preopen.writable {
            (DirPerms::READ | DirPerms::MUTATE, FilePerms::READ | FilePerms::WRITE)
        } else {
            (DirPerms::READ, FilePerms::READ)
        };
        builder
            .preopened_dir(&preopen.host_path, &preopen.guest_path, dir_perms, file_perms)
            .map_err(|err| {
                ToolError::permission_denied(
                    &preopen.host_path,
                    format!("failed to preopen directory for WASI: {err}"),
                )
            })?;
    }

    Ok(builder.build())
}

#[cfg(feature = "host")]
fn path_to_env<'a>(path: &'a Path, name: &str) -> Result<&'a str, ToolError> {
    path.to_str().ok_or_else(|| {
        ToolError::invalid_permission(
            name,
            format!("{name} contains non-UTF-8 bytes and cannot be exposed to WASI"),
        )
    })
}

#[cfg(feature = "host")]
fn guest_path(path: &Path) -> Result<String, ToolError> {
    path.to_str().map(ToOwned::to_owned).ok_or_else(|| {
        ToolError::permission_denied(
            path,
            "preopen path contains non-UTF-8 bytes and cannot be exposed to WASI",
        )
    })
}

#[cfg(feature = "host")]
fn map_guest_error(err: &wasmtime::Error) -> Result<i32, ToolError> {
    if let Some(exit) = err.downcast_ref::<I32Exit>() {
        return Ok(exit.0.clamp(0, 255));
    }
    Err(ToolError::runtime(format!("guest trapped or failed at runtime: {err}")))
}

/// Stub runner used when the `host` Cargo feature is disabled.
///
/// Mirrors the public surface of the wasmtime-backed `WasiRunner` so
/// plan-time helpers build either way; every guest-execution path returns
/// the `tool-host-not-built` diagnostic.
#[cfg(not(feature = "host"))]
#[derive(Debug, Default)]
pub struct WasiRunner {
    _private: (),
}

#[cfg(not(feature = "host"))]
impl WasiRunner {
    /// Construct the stub. Always succeeds; the missing-host diagnostic is
    /// deferred to [`Self::run`] so plan-time helpers still build.
    ///
    /// # Errors
    ///
    /// Never returns an error in this build, but mirrors the host signature.
    pub const fn new() -> Result<Self, ToolError> {
        Ok(Self { _private: () })
    }

    /// Reject the run with the `tool-host-not-built` diagnostic.
    ///
    /// # Errors
    ///
    /// Always returns the `tool-host-not-built` diagnostic.
    pub fn run(&self, _resolved: &ResolvedTool, _ctx: &RunContext) -> Result<i32, ToolError> {
        Err(ToolError::host_not_built())
    }
}

#[cfg(all(test, feature = "host"))]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;
    use crate::manifest::{Tool, ToolPermissions, ToolScope, ToolSource};

    fn tool_with_permissions(read: Vec<String>, write: Vec<String>) -> Tool {
        Tool {
            name: "probe".to_string(),
            version: "1.0.0".to_string(),
            source: ToolSource::LocalPath(PathBuf::from("/tmp/probe.wasm")),
            sha256: None,
            permissions: ToolPermissions { read, write },
        }
    }

    const fn resolved(scope: ToolScope, tool: Tool, bytes_path: PathBuf) -> ResolvedTool {
        ResolvedTool {
            bytes_path,
            scope,
            tool,
        }
    }

    #[test]
    fn prepare_preopens_promotes_write_over_read() {
        let tmp = tempdir().expect("tempdir");
        let project = tmp.path().join("project");
        let output = project.join("output");
        fs::create_dir_all(&output).expect("output");
        let project = project.canonicalize().expect("project");

        let resolved = resolved(
            ToolScope::Project {
                project_name: "demo".to_string(),
            },
            tool_with_permissions(
                vec!["$PROJECT_DIR/output".to_string()],
                vec!["$PROJECT_DIR/output".to_string()],
            ),
            tmp.path().join("missing.wasm"),
        );

        let preopens = prepare_preopens(&resolved, &project, None).expect("preopens");
        assert_eq!(preopens.len(), 1);
        assert!(preopens[0].writable);
        assert_eq!(preopens[0].host_path, output.canonicalize().expect("canonical output"));
    }

    #[test]
    fn run_rejects_capability_dir_in_project_scope_before_loading_component() {
        let tmp = tempdir().expect("tempdir");
        let project = tmp.path().join("project");
        fs::create_dir_all(&project).expect("project");
        let resolved = resolved(
            ToolScope::Project {
                project_name: "demo".to_string(),
            },
            tool_with_permissions(vec!["$CAPABILITY_DIR/templates".to_string()], Vec::new()),
            tmp.path().join("missing.wasm"),
        );

        let runner = WasiRunner::new().expect("runner");
        let err = runner
            .run(&resolved, &RunContext::new(&project, Vec::new()).with_stdio(Stdio::Null))
            .expect_err("permission preparation must fail before component load");
        assert!(matches!(err, ToolError::InvalidPermission { .. }), "{err}");
    }

    #[test]
    fn run_rejects_lifecycle_write_before_loading_component() {
        let tmp = tempdir().expect("tempdir");
        let project = tmp.path().join("project");
        fs::create_dir_all(project.join(".specify")).expect("specify");
        let resolved = resolved(
            ToolScope::Project {
                project_name: "demo".to_string(),
            },
            tool_with_permissions(Vec::new(), vec!["$PROJECT_DIR/.specify".to_string()]),
            tmp.path().join("missing.wasm"),
        );

        let runner = WasiRunner::new().expect("runner");
        let err = runner
            .run(&resolved, &RunContext::new(&project, Vec::new()).with_stdio(Stdio::Null))
            .expect_err("lifecycle permission must fail before component load");
        assert!(matches!(err, ToolError::PermissionDenied { .. }), "{err}");
        assert!(err.to_string().contains("tool.lifecycle-state-write-denied"), "{err}");
    }

    #[test]
    fn invalid_component_bytes_surface_as_runtime_error() {
        let tmp = tempdir().expect("tempdir");
        let project = tmp.path().join("project");
        fs::create_dir_all(&project).expect("project");
        let wasm = tmp.path().join("not-a-component.wasm");
        fs::write(&wasm, b"not wasm").expect("write wasm");
        let resolved = resolved(
            ToolScope::Project {
                project_name: "demo".to_string(),
            },
            tool_with_permissions(Vec::new(), Vec::new()),
            wasm,
        );

        let runner = WasiRunner::new().expect("runner");
        let err = runner
            .run(&resolved, &RunContext::new(&project, Vec::new()).with_stdio(Stdio::Null))
            .expect_err("invalid component must fail");
        assert!(matches!(err, ToolError::Runtime(_)), "{err}");
    }
}
