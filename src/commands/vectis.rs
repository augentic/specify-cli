use serde::Serialize;
use serde_json::Value;

use crate::cli::{OutputFormat, VectisAction};
use crate::output::{CliResult, emit_json};

/// Dispatch one of the four `specify vectis` verbs to the
/// `specify-vectis` library and translate the outcome into the v2
/// contract.
///
/// JSON output goes through [`emit_json`], which auto-injects
/// `schema-version: 2`. Text output is rendered per-verb by the
/// `vectis_text_render_*` helpers below: humanised summaries that match
/// the shapes documented in chunk 5 of
/// `docs/plans/fold-vectis-into-specify.md`. Error variants and the
/// synthesised `not-implemented` shape are kebab-case for JSON and
/// humanised for text.
pub fn run_vectis(format: OutputFormat, action: &VectisAction) -> CliResult {
    let result = match action {
        VectisAction::Init(args) => specify_vectis::init::run(args),
        VectisAction::Verify(args) => specify_vectis::verify::run(args),
        VectisAction::AddShell(args) => specify_vectis::add_shell::run(args),
        VectisAction::UpdateVersions(args) => specify_vectis::update_versions::run(args),
    };
    match result {
        Ok(specify_vectis::CommandOutcome::Success(value)) => {
            match format {
                OutputFormat::Json => emit_json(value),
                OutputFormat::Text => vectis_render_text(action, &value),
            }
            CliResult::Success
        }
        Ok(specify_vectis::CommandOutcome::Stub { command }) => {
            let message = format!("`vectis {command}` is not implemented yet");
            match format {
                OutputFormat::Json => {
                    #[derive(Serialize)]
                    #[serde(rename_all = "kebab-case")]
                    struct NotImplementedResponse<'a> {
                        error: &'static str,
                        command: &'a str,
                        message: String,
                        exit_code: u8,
                    }
                    crate::output::emit_response(NotImplementedResponse {
                        error: "not-implemented",
                        command,
                        message,
                        exit_code: CliResult::GenericFailure.code(),
                    });
                }
                OutputFormat::Text => eprintln!("error: {message}"),
            }
            CliResult::GenericFailure
        }
        Err(err) => emit_vectis_error(format, &err),
        _ => unreachable!(),
    }
}

/// Render a [`specify_vectis::VectisError`] using the v2 contract:
/// kebab-case `error` variant, `message`, and the binary's mapped
/// `exit-code`. The text path renders each variant in a shape an
/// operator can act on without having to re-run with `--format json` —
/// notably, `MissingPrerequisites` lists each missing tool's `tool`,
/// `check`, and `install` on its own line.
///
/// We can't reuse [`emit_json_error`] because that helper is hard-coded
/// against the `specify_error::Error` enum; this is the vectis-shaped
/// sibling.
fn emit_vectis_error(format: OutputFormat, err: &specify_vectis::VectisError) -> CliResult {
    let code = match err {
        specify_vectis::VectisError::MissingPrerequisites { .. } => CliResult::ValidationFailed,
        _ => CliResult::GenericFailure,
    };
    match format {
        OutputFormat::Json => {
            // Single source of truth for the kebab-case `error` variant
            // and per-variant payload shape lives in
            // `VectisError::to_json`; we just splice in the dispatcher's
            // `exit-code` mapping on top so both callers (this helper
            // and any future direct caller of `to_json`) cannot drift.
            let Value::Object(mut payload) = err.to_json() else {
                unreachable!("VectisError::to_json always returns an object")
            };
            payload.entry("exit-code".to_string()).or_insert(Value::from(code.code()));
            emit_json(Value::Object(payload));
        }
        OutputFormat::Text => match err {
            specify_vectis::VectisError::MissingPrerequisites { missing, message } => {
                eprintln!("error: missing prerequisites");
                for tool in missing {
                    eprintln!(
                        "  - {} ({}): {} | install: {}",
                        tool.tool, tool.assembly, tool.check, tool.install
                    );
                }
                eprintln!("{message}");
            }
            _ => {
                eprintln!("error: {err}");
            }
        },
    }
    code
}

/// Dispatch a successful `vectis` payload to the per-verb text renderer.
///
/// The renderers consume the v2 JSON shape directly (rather than the
/// typed result) so this dispatcher does not have to re-thread the four
/// concrete success types out of the library and stays in lock-step
/// with the JSON contract by construction. Defensive `as_*` chains
/// fall back to empty strings/arrays so a future field addition does
/// not panic the text path.
fn vectis_render_text(action: &VectisAction, value: &Value) {
    match action {
        VectisAction::Init(_) => vectis_render_init_text(value),
        VectisAction::Verify(_) => vectis_render_verify_text(value),
        VectisAction::AddShell(_) => vectis_render_add_shell_text(value),
        VectisAction::UpdateVersions(_) => vectis_render_update_versions_text(value),
    }
}

fn vectis_render_init_text(value: &Value) {
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
        // Preserve a stable order: core first, then ios, then android,
        // then anything else alphabetically. Matches the order users
        // see in the JSON envelope.
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
            let build = vectis_render_build_steps_summary(assembly.get("build-steps"));
            match build {
                Some(summary) => println!("  - {key}: {status} ({file_count} files), {summary}"),
                None => println!("  - {key}: {status} ({file_count} files)"),
            }
        }
    }
}

fn vectis_render_verify_text(value: &Value) {
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

fn vectis_render_add_shell_text(value: &Value) {
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
    let build = vectis_render_build_steps_summary(assembly.and_then(|a| a.get("build-steps")));
    match build {
        Some(summary) => println!("Files: {file_count}, {summary}"),
        None => println!("Files: {file_count}"),
    }
}

fn vectis_render_update_versions_text(value: &Value) {
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

/// Summarise a `build-steps` array (init/add-shell shapes) as either
/// "build PASS" or "build FAIL (<first failing step name>)". Returns
/// `None` when no `build-steps` field is present (e.g. the `core`
/// assembly entry from `init`).
fn vectis_render_build_steps_summary(steps: Option<&Value>) -> Option<String> {
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
