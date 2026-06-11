//! `marketplace` framework-authoring WASI tool entrypoint.
//!
//! Run under `specify lint framework`'s `kind: tool` evaluator. The
//! evaluator invokes the tool once per candidate file (a sentinel path,
//! since the marketplace drift check is whole-tree) and reads
//! `PROJECT_DIR` from the environment. The positional path argument names
//! the rule's own sentinel file (`…/CORE-022-…md`); the tool walks the
//! tree itself and carries no policy.
//!
//! Findings are emitted on stdout as the shared
//! [`specify_framework_wire`] `DiagnosticReport` envelope; each carries
//! its own `rule-id: CORE-022` and `severity: important`. The host
//! restamps `id` and `fingerprint`. Exit is always `0` on a successful
//! run: the host treats a non-zero exit with no parsed findings as an
//! invocation failure, so a clean tree must exit `0`.

use std::path::PathBuf;
use std::process::ExitCode;

use specify_framework_wire::{Row, print_report};
use specify_marketplace::{MarketplaceFinding, check_marketplace_drift};

fn main() -> ExitCode {
    let Ok(project_dir) = std::env::var("PROJECT_DIR").map(PathBuf::from) else {
        print_report("marketplace", []);
        return ExitCode::SUCCESS;
    };
    let findings = check_marketplace_drift(&project_dir);
    print_report("marketplace", findings.iter().map(row));
    ExitCode::SUCCESS
}

fn row(finding: &MarketplaceFinding) -> Row<'_> {
    Row {
        rule_id: finding.rule_id,
        message: &finding.message,
        path: finding.path.as_deref(),
        impact: "The marketplace manifest disagrees with the on-disk plugin layout, so the plugin set advertised to Cursor is wrong.",
        remediation: "Reconcile .cursor-plugin/marketplace.json with the plugins/ tree: declare every on-disk plugin and ensure each declared plugin has skills/ and a plugin.json.",
    }
}
