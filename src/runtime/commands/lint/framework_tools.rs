//! Name-resolved tool runner for `specify lint framework` (Road B; see
//! DECISIONS.md §"Framework lint engine: generic dispatcher (Road A / Road B)").
//!
//! Framework runs have no `project.yaml` to populate a tool inventory, so
//! the first-party framework/authoring checkers are declared here as a
//! closed, embedded inventory keyed by name. `is_declared` / `run`
//! dispatch by name — the standards engine never calls the imperative
//! check in-process, which is the decoupling lever the plan requires.
//!
//! Interim posture (B-2): the `.wasm` artifact is built from
//! `wasi-tools/scenarios/` and embedded into this binary, so a framework
//! `kind: tool` rule resolves with no separate build step at lint time and
//! no digest pinning yet. The artifact lives in `specify-cli` only as
//! explicitly temporary **versioned-artifact coupling**: nothing here
//! reimplements rule logic or bakes rule policy into the engine — it stages
//! a generic component and runs it by name. Exit condition: when the tool
//! source moves to its colocated framework-tools home, this inventory
//! switches to `specify_tool::resolver::resolve` with a pinned `sha256`,
//! leaving the rule files, the engine, and the parity tests untouched.

use std::path::{Path, PathBuf};

use specify_error::{Error, Result};
use specify_standards::lint::eval::tool::{ToolOutput, ToolRunError, ToolRunner};
use specify_tool::host::{RunContext, WasiRunner};
use specify_tool::manifest::{Tool, ToolPermissions, ToolScope, ToolSource};
use specify_tool::resolver::ResolvedTool;

/// `scenarios` framework checker component, built from
/// `wasi-tools/scenarios/` via `cargo make scenarios-wasm` (Road B
/// scenario family: CORE-028, 029, 031, 033; CORE-030 and CORE-032
/// moved to Road A `scenario`-fact hints).
const SCENARIOS_WASM: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/wasi-tools/scenarios/dist/scenarios-0.1.0.wasm"
));

/// `skill-body` framework checker component, built from
/// `wasi-tools/skill-body/` via `cargo make skill-body-wasm` (Road B
/// skill body family: CORE-040, 046, 048; CORE-041 moved to Road A
/// `kind: presence` `markdown-section`).
const SKILL_BODY_WASM: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/wasi-tools/skill-body/dist/skill-body-0.1.0.wasm"
));

/// `agent-teams` framework checker component, built from
/// `wasi-tools/agent-teams/` via `cargo make agent-teams-wasm` (Road B
/// agent-teams overlay drift: CORE-012; CORE-011 moved to Road A
/// `kind: presence` `file`).
const AGENT_TEAMS_WASM: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/wasi-tools/agent-teams/dist/agent-teams-0.1.0.wasm"
));

/// `links-registry` framework checker component, built from
/// `wasi-tools/links-registry/` via `cargo make links-registry-wasm`
/// (Road B link-registry family: CORE-018, 020).
const LINKS_REGISTRY_WASM: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/wasi-tools/links-registry/dist/links-registry-0.1.0.wasm"
));

/// `marketplace` framework checker component, built from
/// `wasi-tools/marketplace/` via `cargo make marketplace-wasm` (Road B
/// marketplace drift: CORE-022).
const MARKETPLACE_WASM: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/wasi-tools/marketplace/dist/marketplace-0.1.0.wasm"
));

/// `prose` framework checker component, built from `wasi-tools/prose/`
/// via `cargo make prose-wasm` (Road B numeric-cap scan: CORE-024).
const PROSE_WASM: &[u8] =
    include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/wasi-tools/prose/dist/prose-0.1.0.wasm"));

/// `rules` framework checker component, built from `wasi-tools/rules/`
/// via `cargo make rules-wasm` (Road B rule-tree family: CORE-009
/// namespace ownership, CORE-026 duplicate rule id).
const RULES_WASM: &[u8] =
    include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/wasi-tools/rules/dist/rules-0.1.0.wasm"));

/// One first-party framework checker: its declared name, version, and
/// embedded component bytes.
struct FrameworkTool {
    name: &'static str,
    version: &'static str,
    bytes: &'static [u8],
}

/// Closed inventory of framework checkers `specify lint framework`
/// resolves by name. Grows one row per Road B family tool.
const FRAMEWORK_TOOLS: &[FrameworkTool] = &[
    FrameworkTool {
        name: "scenarios",
        version: "0.1.0",
        bytes: SCENARIOS_WASM,
    },
    FrameworkTool {
        name: "skill-body",
        version: "0.1.0",
        bytes: SKILL_BODY_WASM,
    },
    FrameworkTool {
        name: "agent-teams",
        version: "0.1.0",
        bytes: AGENT_TEAMS_WASM,
    },
    FrameworkTool {
        name: "links-registry",
        version: "0.1.0",
        bytes: LINKS_REGISTRY_WASM,
    },
    FrameworkTool {
        name: "marketplace",
        version: "0.1.0",
        bytes: MARKETPLACE_WASM,
    },
    FrameworkTool {
        name: "prose",
        version: "0.1.0",
        bytes: PROSE_WASM,
    },
    FrameworkTool {
        name: "rules",
        version: "0.1.0",
        bytes: RULES_WASM,
    },
];

fn lookup(name: &str) -> Option<&'static FrameworkTool> {
    FRAMEWORK_TOOLS.iter().find(|tool| tool.name == name)
}

/// Name-resolving [`ToolRunner`] for the framework surface.
#[derive(Debug)]
pub struct FrameworkToolRunner {
    runner: WasiRunner,
    staging_dir: PathBuf,
}

impl FrameworkToolRunner {
    /// Build the runner and its staging directory under the OS temp dir.
    ///
    /// # Errors
    ///
    /// Propagates Wasmtime engine construction failure and staging-dir
    /// creation failure.
    pub fn new() -> Result<Self> {
        let runner = WasiRunner::new().map_err(|err| Error::Diag {
            code: "framework-tool-runner",
            detail: err.to_string(),
        })?;
        let staging_dir = std::env::temp_dir().join("specify-framework-tools");
        std::fs::create_dir_all(&staging_dir).map_err(|source| Error::Filesystem {
            op: "framework-tool-staging",
            path: staging_dir.clone(),
            source,
        })?;
        Ok(Self { runner, staging_dir })
    }

    /// Write a framework tool's embedded bytes to a content-stable path
    /// and return it. Writes through a per-process temp name then renames
    /// so a concurrent framework run never observes a partial component.
    fn stage(&self, tool: &FrameworkTool) -> std::result::Result<PathBuf, ToolRunError> {
        let dest = self.staging_dir.join(format!("{}-{}.wasm", tool.name, tool.version));
        if std::fs::metadata(&dest)
            .is_ok_and(|meta| usize::try_from(meta.len()).is_ok_and(|len| len == tool.bytes.len()))
        {
            return Ok(dest);
        }
        let tmp = self.staging_dir.join(format!(
            "{}-{}.wasm.{}.tmp",
            tool.name,
            tool.version,
            std::process::id()
        ));
        std::fs::write(&tmp, tool.bytes)
            .and_then(|()| std::fs::rename(&tmp, &dest))
            .map_err(|err| ToolRunError::Runtime(format!("stage {}: {err}", tool.name)))?;
        Ok(dest)
    }
}

impl ToolRunner for FrameworkToolRunner {
    fn is_declared(&self, tool_name: &str) -> bool {
        lookup(tool_name).is_some()
    }

    fn run(
        &self, tool_name: &str, args: &[String], project_dir: &Path,
    ) -> std::result::Result<ToolOutput, ToolRunError> {
        let Some(tool) = lookup(tool_name) else {
            return Err(ToolRunError::Runtime(format!(
                "tool {tool_name} is not a declared framework checker"
            )));
        };
        let bytes_path = self.stage(tool)?;
        let resolved = ResolvedTool {
            bytes_path: bytes_path.clone(),
            scope: ToolScope::Project {
                project_name: "specify-framework".to_string(),
            },
            tool: Tool {
                name: tool.name.to_string(),
                version: tool.version.to_string(),
                source: ToolSource::LocalPath(bytes_path),
                sha256: None,
                permissions: ToolPermissions {
                    read: vec!["$PROJECT_DIR".to_string()],
                    write: Vec::new(),
                },
            },
        };
        let run_ctx = RunContext::new(project_dir, args.to_vec());
        let captured = self
            .runner
            .run_captured(&resolved, &run_ctx)
            .map_err(|err| ToolRunError::Runtime(err.to_string()))?;
        Ok(ToolOutput {
            stdout: captured.stdout,
            stderr: captured.stderr,
            exit_code: captured.exit_code,
        })
    }
}
