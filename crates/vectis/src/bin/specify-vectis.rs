//! `specify-vectis` — standalone binary for the Vectis capability's
//! deterministic verbs (RFC-13 §4.3a).
//!
//! Wraps `crates/vectis`'s library API. Five verbs are exposed, each a
//! one-line dispatch into a handler that already exists in the library
//! (`specify_vectis::{init, verify, add_shell, update_versions,
//! versions_cmd}::run`):
//!
//! * `init`              — scaffold a new Crux project (core + optional shells).
//! * `verify`            — run the per-assembly compilation pipelines.
//! * `add-shell`         — append an iOS or Android shell to an existing core.
//! * `update-versions`   — query registries and refresh the pinned tool/crate
//!   versions, optionally proving the cap-matrix builds first.
//! * `versions`          — print the resolved version pins (embedded → user →
//!   project → `--version-file` override).
//!
//! The JSON envelope (default `--format json`) is byte-for-byte the
//! same shape the pre-2.6 `specify vectis * --format json` dispatcher
//! produced (RFC-13 phase 4.3a's parity contract): `schema-version: 2`
//! first, then the per-verb payload, kebab-case keys throughout.
//! Operator scripts that parsed the legacy output keep working without
//! modification.
//!
//! The library API stays first-class: capability skills that prefer
//! to call in-process can still invoke the per-verb `run` functions
//! directly, take ownership of the returned [`specify_vectis::CommandOutcome`],
//! and forward via [`specify_vectis::render_envelope_json`] when they
//! need the same JSON envelope this binary emits.
//!
//! # Exit codes
//!
//! Mirror [`specify_vectis::VectisError::exit_code`]:
//!
//! - `0` — success.
//! - `1` — generic failure (`io`, `invalid-project`, `verify`, `internal`,
//!   `not-implemented`).
//! - `2` — `missing-prerequisites` (workstation toolchain incomplete).

use std::process::ExitCode;

use clap::{Parser, Subcommand, ValueEnum};
use serde_json::Value;
use specify_vectis::{
    AddShellArgs, CommandOutcome, InitArgs, JSON_SCHEMA_VERSION, UpdateVersionsArgs, VectisError,
    VerifyArgs, VersionsArgs, render_envelope_json,
};

/// Top-level `specify-vectis` CLI.
#[derive(Parser, Debug)]
#[command(
    name = "specify-vectis",
    version,
    about = "Vectis Crux scaffolding, verification, and version management (RFC-13 §4.3a).",
    long_about = "Standalone binary for the Vectis capability's deterministic verbs. \
                  Wraps `crates/vectis`'s library API; the same handlers run when a \
                  capability skill calls in-process.\n\
                  \nVerbs:\n\
                  \n  \
                  init               scaffold a new Crux project (core + optional shells)\n  \
                  verify             run the per-assembly compilation pipelines\n  \
                  add-shell          append an iOS or Android shell to an existing core\n  \
                  update-versions    query registries and refresh pinned tool / crate versions\n  \
                  versions           print the resolved version pins\n\
                  \nJSON output (`--format json`, default) follows the v2 contract used by \
                  the pre-2.6 `specify vectis * --format json` dispatcher byte-for-byte: \
                  `schema-version: 2` first, then the per-verb payload, kebab-case keys \
                  throughout. Errors become `{error, message, exit-code, ...}` with \
                  variant-specific extras (`missing` for `missing-prerequisites`).\n\
                  \nExit codes: 0 success / 1 generic failure / 2 missing prerequisites."
)]
struct Cli {
    /// Subcommand to dispatch.
    #[command(subcommand)]
    command: Command,

    /// Output format.
    ///
    /// `json` (default) emits the v2 envelope this binary's JSON
    /// parity contract pins; `text` emits a humanised per-verb summary
    /// to stdout (and stderr for errors).
    #[arg(long, value_enum, default_value_t = OutputFormat::Json, global = true)]
    format: OutputFormat,
}

/// Output format selector for the global `--format` flag.
#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
enum OutputFormat {
    /// Pretty-printed v2 JSON envelope (default).
    Json,
    /// Humanised per-verb summary on stdout; errors on stderr.
    Text,
}

/// `specify-vectis` subcommand surface. Each variant flattens the
/// matching `clap::Args` struct from [`specify_vectis`], so flag
/// parsing stays in lock-step with the library's own arg definitions.
#[derive(Subcommand, Debug)]
enum Command {
    /// Scaffold a new Crux project (core + optional shells).
    Init(InitArgs),
    /// Verify that a Crux project still builds end-to-end.
    Verify(VerifyArgs),
    /// Add an iOS or Android shell to an existing core.
    AddShell(AddShellArgs),
    /// Refresh pinned tool/crate versions and (optionally) verify them.
    UpdateVersions(UpdateVersionsArgs),
    /// Show the resolved version pins (embedded → user → project → override).
    Versions(VersionsArgs),
}

impl Command {
    /// Dispatch into the library's per-verb handler.
    fn run(&self) -> Result<CommandOutcome, VectisError> {
        match self {
            Self::Init(args) => specify_vectis::init::run(args),
            Self::Verify(args) => specify_vectis::verify::run(args),
            Self::AddShell(args) => specify_vectis::add_shell::run(args),
            Self::UpdateVersions(args) => specify_vectis::update_versions::run(args),
            Self::Versions(args) => specify_vectis::versions_cmd::run(args),
        }
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let outcome = cli.command.run();

    match cli.format {
        OutputFormat::Json => {
            let (json, code) = render_envelope_json(outcome);
            println!("{json}");
            ExitCode::from(code)
        }
        OutputFormat::Text => render_text(&cli.command, outcome),
    }
}

/// Render `outcome` in human-readable text. Returns the exit code the
/// process should exit with — matches the JSON path so a caller cannot
/// observe a different status by toggling `--format`.
///
/// The per-verb renderers below consume the v2 JSON shape produced by
/// the handlers (rather than the typed result types) so this dispatcher
/// does not have to re-thread the four concrete success types out of
/// the library and stays in lock-step with the JSON contract by
/// construction. Defensive `as_*` chains fall back to empty
/// strings/arrays so a future field addition does not panic the text
/// path.
fn render_text(command: &Command, outcome: Result<CommandOutcome, VectisError>) -> ExitCode {
    match outcome {
        Ok(CommandOutcome::Success(value)) => {
            match command {
                Command::Init(_) => render_init_text(&value),
                Command::Verify(_) => render_verify_text(&value),
                Command::AddShell(_) => render_add_shell_text(&value),
                Command::UpdateVersions(_) => render_update_versions_text(&value),
                Command::Versions(_) => render_versions_text(&value),
            }
            ExitCode::from(0)
        }
        Ok(CommandOutcome::Stub { command: verb }) => {
            eprintln!("error: `vectis {verb}` is not implemented yet");
            ExitCode::from(1)
        }
        Err(err) => render_error_text(&err),
        // `CommandOutcome` is `#[non_exhaustive]`; cover any future
        // variant by failing loudly rather than silently exiting 0.
        Ok(_) => {
            eprintln!("error: unhandled CommandOutcome variant");
            ExitCode::from(1)
        }
    }
}

fn render_error_text(err: &VectisError) -> ExitCode {
    let code = u8::try_from(err.exit_code()).unwrap_or(1);
    match err {
        VectisError::MissingPrerequisites { missing, message } => {
            eprintln!("error: missing prerequisites");
            for tool in missing {
                if let Some(reason) = &tool.reason {
                    eprintln!(
                        "  - {} ({}): {} — {} | install: {}",
                        tool.tool, tool.assembly, tool.check, reason, tool.install
                    );
                } else {
                    eprintln!(
                        "  - {} ({}): {} | install: {}",
                        tool.tool, tool.assembly, tool.check, tool.install
                    );
                }
            }
            eprintln!("{message}");
        }
        _ => {
            eprintln!("error: {err}");
        }
    }
    ExitCode::from(code)
}

fn render_init_text(value: &Value) {
    let app = value.get("app-name").and_then(Value::as_str).unwrap_or("<app>");
    let dir = value.get("project-dir").and_then(Value::as_str).unwrap_or("<dir>");
    println!("Created app \"{app}\" at {dir}");

    let caps: Vec<&str> = value
        .get("capabilities")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    if caps.is_empty() {
        println!("Capabilities: (none)");
    } else {
        println!("Capabilities: {}", caps.join(", "));
    }

    println!("Assemblies:");
    if let Some(map) = value.get("assemblies").and_then(Value::as_object) {
        // Stable order: core first, then ios, then android, then anything
        // else alphabetically. Matches the order users see in the JSON
        // envelope.
        let mut keys: Vec<&String> = map.keys().collect();
        keys.sort_by_key(|k| match k.as_str() {
            "core" => (0, String::new()),
            "ios" => (1, String::new()),
            "android" => (2, String::new()),
            other => (3, other.to_string()),
        });
        for key in keys {
            let assembly = &map[key];
            let status = assembly.get("status").and_then(Value::as_str).unwrap_or("?");
            let file_count = assembly.get("files").and_then(Value::as_array).map_or(0, Vec::len);
            let build = render_build_steps_summary(assembly.get("build-steps"));
            match build {
                Some(summary) => println!("  - {key}: {status} ({file_count} files), {summary}"),
                None => println!("  - {key}: {status} ({file_count} files)"),
            }
        }
    }
}

fn render_verify_text(value: &Value) {
    let dir = value.get("project-dir").and_then(Value::as_str).unwrap_or("<dir>");
    let passed = value.get("passed").and_then(Value::as_bool).unwrap_or(false);
    println!("Verified {dir}: {}", if passed { "PASS" } else { "FAIL" });
    if let Some(map) = value.get("assemblies").and_then(Value::as_object) {
        let mut keys: Vec<&String> = map.keys().collect();
        keys.sort_by_key(|k| match k.as_str() {
            "core" => (0, String::new()),
            "ios" => (1, String::new()),
            "android" => (2, String::new()),
            other => (3, other.to_string()),
        });
        for key in keys {
            let assembly = &map[key];
            let assembly_passed = assembly.get("passed").and_then(Value::as_bool).unwrap_or(false);
            println!("  - {key}: {}", if assembly_passed { "PASS" } else { "FAIL" });
            if !assembly_passed && let Some(steps) = assembly.get("steps").and_then(Value::as_array)
            {
                for step in steps {
                    let name = step.get("name").and_then(Value::as_str).unwrap_or("?");
                    let step_passed = step.get("passed").and_then(Value::as_bool).unwrap_or(false);
                    println!("      - {name}: {}", if step_passed { "PASS" } else { "FAIL" });
                    if !step_passed
                        && let Some(err) = step.get("error").and_then(Value::as_str)
                        && let Some(first) = err.lines().find(|l| !l.trim().is_empty())
                    {
                        println!("        error: {first}");
                    }
                }
            }
        }
    }
}

fn render_add_shell_text(value: &Value) {
    let app = value.get("app-name").and_then(Value::as_str).unwrap_or("<app>");
    let dir = value.get("project-dir").and_then(Value::as_str).unwrap_or("<dir>");
    let platform = value.get("platform").and_then(Value::as_str).unwrap_or("<platform>");
    println!("Added {platform} shell to \"{app}\" at {dir}");

    let detected: Vec<&str> = value
        .get("detected-capabilities")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    if detected.is_empty() {
        println!("Detected capabilities: (none)");
    } else {
        println!("Detected capabilities: {}", detected.join(", "));
    }
    let unrecognized: Vec<&str> = value
        .get("unrecognized-capabilities")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    if !unrecognized.is_empty() {
        println!("Unrecognized capabilities: {}", unrecognized.join(", "));
    }

    let assembly = value.get("assembly");
    let file_count =
        assembly.and_then(|a| a.get("files")).and_then(Value::as_array).map_or(0, Vec::len);
    let build = render_build_steps_summary(assembly.and_then(|a| a.get("build-steps")));
    match build {
        Some(summary) => println!("Files: {file_count}, {summary}"),
        None => println!("Files: {file_count}"),
    }
}

fn render_update_versions_text(value: &Value) {
    let target = value.get("version-file").and_then(Value::as_str).unwrap_or("<file>");
    let dry_run = value.get("dry-run").and_then(Value::as_bool).unwrap_or(false);
    let written = value.get("written").and_then(Value::as_bool).unwrap_or(false);
    let mode = if dry_run {
        " (dry-run)"
    } else if written {
        " (written)"
    } else {
        " (no write)"
    };
    println!("Versions file: {target}{mode}");

    let changes = value.get("changes").and_then(Value::as_array).cloned().unwrap_or_default();
    if changes.is_empty() {
        println!("No changes.");
    } else {
        println!("Changes:");
        for c in &changes {
            let key = c.get("key").and_then(Value::as_str).unwrap_or("?");
            let cur = c.get("current").and_then(Value::as_str).unwrap_or("?");
            let prop = c.get("proposed").and_then(Value::as_str).unwrap_or("?");
            println!("  - {key}: {cur} → {prop}");
        }
    }

    if let Some(errors) = value.get("errors").and_then(Value::as_array)
        && !errors.is_empty()
    {
        println!("Errors:");
        for e in errors {
            if let Some(s) = e.as_str() {
                println!("  - {s}");
            }
        }
    }

    if let Some(verification) = value.get("verification") {
        let passed = verification.get("passed").and_then(Value::as_bool).unwrap_or(false);
        println!("Verify matrix: {}", if passed { "PASS" } else { "FAIL" });
        if let Some(combos) = verification.get("combos").and_then(Value::as_array) {
            for combo in combos {
                let caps = combo.get("caps").and_then(Value::as_str).unwrap_or("?");
                let combo_passed = combo.get("passed").and_then(Value::as_bool).unwrap_or(false);
                println!("  - {caps}: {}", if combo_passed { "PASS" } else { "FAIL" });
            }
        }
    }
}

fn render_versions_text(value: &Value) {
    println!("Resolved version pins:");
    let sections = ["crux", "android", "ios", "tooling"];
    for section in &sections {
        if let Some(obj) = value.get(section).and_then(Value::as_object) {
            if obj.is_empty() {
                continue;
            }
            println!("  [{section}]");
            let mut keys: Vec<&String> = obj.keys().collect();
            keys.sort();
            for key in keys {
                let val = obj.get(key).and_then(Value::as_str).unwrap_or("?");
                println!("    {key} = {val}");
            }
        }
    }
}

/// Summarise a `build-steps` array (init/add-shell shapes) as either
/// "build PASS" or "build FAIL (<first failing step name>)". Returns
/// `None` when no `build-steps` field is present (e.g. the `core`
/// assembly entry from `init`).
fn render_build_steps_summary(steps: Option<&Value>) -> Option<String> {
    let arr = steps?.as_array()?;
    if arr.is_empty() {
        return Some("build PASS".to_string());
    }
    for step in arr {
        let passed = step.get("passed").and_then(Value::as_bool).unwrap_or(false);
        if !passed {
            let name = step.get("name").and_then(Value::as_str).unwrap_or("?");
            return Some(format!("build FAIL ({name})"));
        }
    }
    Some("build PASS".to_string())
}

/// Compile-time assertion that the binary's envelope schema version
/// stays pinned to the v2 contract — any drift here is a breaking
/// change for skill authors and forces a deliberate bump.
const _: () = assert!(JSON_SCHEMA_VERSION == 2);
