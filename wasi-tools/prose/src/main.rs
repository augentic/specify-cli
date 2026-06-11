//! `prose` framework-authoring WASI tool entrypoint.
//!
//! Run under `specify lint framework`'s `kind: tool` evaluator. The
//! evaluator invokes the tool once per candidate file (a sentinel path,
//! since the numeric-cap scan is whole-tree) and reads `PROJECT_DIR` from
//! the environment. The positional args carry the rule's own sentinel
//! path (`…/CORE-024-…md`) and its `config:` serialised as JSON; the tool
//! reads the description / body cap *values* from the forwarded config,
//! so no rule-specific cap is baked into this binary.
//!
//! Findings are emitted on stdout as the shared
//! [`specify_framework_wire`] `DiagnosticReport` envelope; each carries
//! its own `rule-id: CORE-024` and `severity: important`. The host
//! restamps `id` and `fingerprint`. Exit is always `0` on a successful
//! run: the host treats a non-zero exit with no parsed findings as an
//! invocation failure, so a clean tree must exit `0`.

use std::path::PathBuf;
use std::process::ExitCode;

use serde_json::Value as JsonValue;
use specify_framework_wire::{Row, parsed_config, print_report};
use specify_prose::{ProseFinding, check_numeric_caps};

fn main() -> ExitCode {
    let Ok(project_dir) = std::env::var("PROJECT_DIR").map(PathBuf::from) else {
        print_report("prose", []);
        return ExitCode::SUCCESS;
    };
    let args: Vec<String> = std::env::args().collect();
    let config = parsed_config(&args);
    let Some((description_cap, body_cap)) = caps(config.as_ref()) else {
        // No policy supplied: nothing to compare against. Emit a clean
        // report rather than inventing a cap.
        print_report("prose", []);
        return ExitCode::SUCCESS;
    };
    let findings = check_numeric_caps(&project_dir, description_cap, body_cap);
    print_report("prose", findings.iter().map(row));
    ExitCode::SUCCESS
}

/// Read CORE-024's `{description-cap, body-cap}` policy out of the
/// forwarded config; `None` when either is absent.
fn caps(config: Option<&JsonValue>) -> Option<(u64, u64)> {
    let config = config?;
    let description = config.get("description-cap").and_then(JsonValue::as_u64)?;
    let body = config.get("body-cap").and_then(JsonValue::as_u64)?;
    Some((description, body))
}

fn row(finding: &ProseFinding) -> Row<'_> {
    Row {
        rule_id: finding.rule_id,
        message: &finding.message,
        path: finding.path.as_deref(),
        impact: "A documented numeric cap has drifted from its canonical source, so authors read a stale limit.",
        remediation: "Restore the cap value in the drifted source so the schema and standards doc agree with the rule's policy.",
    }
}
