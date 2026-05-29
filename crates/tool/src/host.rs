//! Wasmtime-backed WASI Preview 2 runner boundary.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use wasmtime::component::{Component, Linker, ResourceTable};
use wasmtime::{Config, Engine, Store};
use wasmtime_wasi::p2::bindings::sync::Command;
use wasmtime_wasi::p2::pipe::MemoryOutputPipe;
use wasmtime_wasi::{DirPerms, FilePerms, I32Exit, WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};

use crate::error::ToolError;
use crate::manifest::ToolScope;
use crate::permissions::{canonicalise_under, deny_lifecycle_write, substitute};
use crate::resolver::ResolvedTool;

/// Host-side context for running a resolved tool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunContext {
    /// Project root used for `$PROJECT_DIR` and permission-root checks.
    pub project_dir: PathBuf,
    /// Canonical or canonicalisable capability root for plugin-scope tools.
    pub capability_dir: Option<PathBuf>,
    /// Arguments forwarded after `argv[0]`, which is always the tool name.
    pub args: Vec<String>,
}

impl RunContext {
    /// Construct a run context with inherited stdio.
    #[must_use]
    pub fn new(project_dir: impl Into<PathBuf>, args: Vec<String>) -> Self {
        Self {
            project_dir: project_dir.into(),
            capability_dir: None,
            args,
        }
    }

    /// Attach a capability root for plugin-scope tools.
    #[must_use]
    pub fn with_capability_dir(mut self, capability_dir: impl Into<PathBuf>) -> Self {
        self.capability_dir = Some(capability_dir.into());
        self
    }
}

/// Wasmtime-backed synchronous WASI Preview 2 runner.
pub struct WasiRunner {
    engine: Engine,
}

impl std::fmt::Debug for WasiRunner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // `wasmtime::Engine` is not `Debug`; expose the wrapper shape only.
        f.debug_struct("WasiRunner").finish_non_exhaustive()
    }
}

impl WasiRunner {
    /// Construct a reusable Wasmtime engine for WASI Preview 2 components.
    ///
    /// # Errors
    ///
    /// Returns a runtime error when the engine configuration cannot be built.
    pub fn new() -> Result<Self, ToolError> {
        let mut config = Config::new();
        config.wasm_component_model(true);
        let engine = Engine::new(&config).map_err(|err| ToolError::Diag {
            code: "tool-runtime",
            detail: format!("failed to create Wasmtime engine: {err}"),
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
        self.invoke(resolved, ctx, Stdio::Inherit).map(|outcome| outcome.exit_code)
    }

    /// Run a resolved WASI tool with stdout / stderr captured in memory.
    ///
    /// Mirrors [`Self::run`] but redirects the guest's stdout and stderr
    /// into capped [`MemoryOutputPipe`] buffers so the host can examine
    /// the output without printing to the inherited terminal. Used by
    /// `specrun lint`'s `kind: tool` evaluator to fold a tool's
    /// `LintResult` envelope into the scan output (`kind: tool` evaluator contract).
    ///
    /// # Errors
    ///
    /// Same failure modes as [`Self::run`]; the captured buffers are
    /// dropped on error.
    pub fn run_captured(
        &self, resolved: &ResolvedTool, ctx: &RunContext,
    ) -> Result<CapturedOutput, ToolError> {
        let stdout = MemoryOutputPipe::new(CAPTURE_BUFFER_BYTES);
        let stderr = MemoryOutputPipe::new(CAPTURE_BUFFER_BYTES);
        let outcome =
            self.invoke(resolved, ctx, Stdio::Captured(stdout.clone(), stderr.clone()))?;
        Ok(CapturedOutput {
            stdout: stdout.contents().to_vec(),
            stderr: stderr.contents().to_vec(),
            exit_code: outcome.exit_code,
        })
    }

    fn invoke(
        &self, resolved: &ResolvedTool, ctx: &RunContext, stdio: Stdio,
    ) -> Result<Outcome, ToolError> {
        let project_dir = canonical_project_dir(&ctx.project_dir)?;
        let capability_dir =
            canonical_capability_dir(&resolved.scope, ctx.capability_dir.as_deref())?;
        let preopens = prepare_preopens(resolved, &project_dir, capability_dir.as_deref())?;
        let wasi = build_wasi_ctx(
            resolved,
            ctx,
            &project_dir,
            capability_dir.as_deref(),
            &preopens,
            stdio,
        )?;

        let component =
            Component::from_file(&self.engine, &resolved.bytes_path).map_err(|err| {
                ToolError::Diag {
                    code: "tool-runtime",
                    detail: format!("failed to compile component: {err}"),
                }
            })?;
        let mut linker = Linker::<WasiState>::new(&self.engine);
        let mut link_options = wasmtime_wasi::p2::bindings::sync::LinkOptions::default();
        link_options.cli_exit_with_code(true);
        wasmtime_wasi::p2::add_to_linker_with_options_sync(&mut linker, &link_options).map_err(
            |err| ToolError::Diag {
                code: "tool-runtime",
                detail: format!("failed to link WASI Preview 2: {err}"),
            },
        )?;
        let mut store = Store::new(
            &self.engine,
            WasiState {
                ctx: wasi,
                table: ResourceTable::new(),
            },
        );
        let command = Command::instantiate(&mut store, &component, &linker).map_err(|err| {
            ToolError::Diag {
                code: "tool-runtime",
                detail: format!("failed to instantiate command: {err}"),
            }
        })?;

        let exit_code = match command.wasi_cli_run().call_run(&mut store) {
            Ok(Ok(())) => 0,
            Ok(Err(())) => 1,
            Err(err) => map_guest_error(&err)?,
        };
        Ok(Outcome { exit_code })
    }
}

/// Captured stdout / stderr bytes plus the guest exit code from a
/// [`WasiRunner::run_captured`] invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapturedOutput {
    /// Verbatim stdout bytes captured from the guest.
    pub stdout: Vec<u8>,
    /// Verbatim stderr bytes captured from the guest.
    pub stderr: Vec<u8>,
    /// Process-style exit code returned by the guest.
    pub exit_code: i32,
}

const CAPTURE_BUFFER_BYTES: usize = 1024 * 1024;

#[derive(Debug)]
enum Stdio {
    Inherit,
    Captured(MemoryOutputPipe, MemoryOutputPipe),
}

#[derive(Debug, Clone, Copy)]
struct Outcome {
    exit_code: i32,
}

struct WasiState {
    ctx: WasiCtx,
    table: ResourceTable,
}

impl WasiView for WasiState {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.ctx,
            table: &mut self.table,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Preopen {
    host_path: PathBuf,
    guest_path: String,
    writable: bool,
}

fn canonical_project_dir(project_dir: &Path) -> Result<PathBuf, ToolError> {
    project_dir.canonicalize().map_err(|err| {
        ToolError::permission_denied(
            project_dir,
            format!("PROJECT_DIR must exist and be canonicalisable: {err}"),
        )
    })
}

fn canonical_capability_dir(
    scope: &ToolScope, ctx_capability_dir: Option<&Path>,
) -> Result<Option<PathBuf>, ToolError> {
    match scope {
        ToolScope::Project { .. } => Ok(None),
        ToolScope::Plugin { capability_dir, .. } => {
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

fn build_wasi_ctx(
    resolved: &ResolvedTool, ctx: &RunContext, project_dir: &Path, capability_dir: Option<&Path>,
    preopens: &[Preopen], stdio: Stdio,
) -> Result<WasiCtx, ToolError> {
    let mut builder = WasiCtxBuilder::new();
    builder
        .allow_blocking_current_thread(true)
        .allow_tcp(false)
        .allow_udp(false)
        .allow_ip_name_lookup(false);

    match stdio {
        Stdio::Inherit => {
            builder.inherit_stdio();
        }
        Stdio::Captured(stdout, stderr) => {
            builder.stdout(stdout);
            builder.stderr(stderr);
        }
    }

    let mut argv = Vec::with_capacity(ctx.args.len() + 1);
    argv.push(resolved.tool.name.clone());
    argv.extend(ctx.args.iter().cloned());
    builder.args(&argv);

    builder.env(
        "PROJECT_DIR",
        project_dir.to_str().ok_or_else(|| {
            ToolError::invalid_permission(
                "PROJECT_DIR",
                "PROJECT_DIR contains non-UTF-8 bytes and cannot be exposed to WASI",
            )
        })?,
    );
    if let Some(capability_dir) = capability_dir {
        builder.env(
            "CAPABILITY_DIR",
            capability_dir.to_str().ok_or_else(|| {
                ToolError::invalid_permission(
                    "CAPABILITY_DIR",
                    "CAPABILITY_DIR contains non-UTF-8 bytes and cannot be exposed to WASI",
                )
            })?,
        );
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

fn guest_path(path: &Path) -> Result<String, ToolError> {
    path.to_str().map(ToOwned::to_owned).ok_or_else(|| {
        ToolError::permission_denied(
            path,
            "preopen path contains non-UTF-8 bytes and cannot be exposed to WASI",
        )
    })
}

fn map_guest_error(err: &wasmtime::Error) -> Result<i32, ToolError> {
    if let Some(exit) = err.downcast_ref::<I32Exit>() {
        return Ok(exit.0.clamp(0, 255));
    }
    Err(ToolError::Diag {
        code: "tool-runtime",
        detail: format!("guest trapped or failed at runtime: {err}"),
    })
}

#[cfg(test)]
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
    fn run_rejects_cap_dir_in_project_scope() {
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
            .run(&resolved, &RunContext::new(&project, Vec::new()))
            .expect_err("permission preparation must fail before component load");
        assert!(matches!(err, ToolError::InvalidPermission { .. }), "{err}");
    }

    #[test]
    fn run_rejects_lifecycle_write() {
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
            .run(&resolved, &RunContext::new(&project, Vec::new()))
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
            .run(&resolved, &RunContext::new(&project, Vec::new()))
            .expect_err("invalid component must fail");
        assert!(
            matches!(
                err,
                ToolError::Diag {
                    code: "tool-runtime",
                    ..
                }
            ),
            "{err}"
        );
    }
}
