use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;
use specify_diagnostics::Diagnostic;

use crate::framework::builder::{framework_finding, loc};
use crate::framework::check::Check;
use crate::framework::context::Context;

const RULE_INVALID_DECLARATION: &str = "tools.invalid-declaration";
struct ExpectedToolDeclaration {
    adapter: &'static str,
    name: &'static str,
    package: &'static str,
}

const EXPECTED_FIRST_PARTY_TOOLS: &[ExpectedToolDeclaration] = &[
    ExpectedToolDeclaration {
        adapter: "contracts",
        name: "contract",
        package: "specify:contract@0.3.0",
    },
    ExpectedToolDeclaration {
        adapter: "vectis",
        name: "vectis",
        package: "specify:vectis@0.4.0",
    },
];

/// Validate first-party WASM tool declarations in target adapter manifests.
pub struct FirstPartyTools;

impl Check for FirstPartyTools {
    fn run(&self, ctx: &Context) -> Vec<Diagnostic> {
        check_first_party_tools(ctx)
    }
}

/// Run first-party tool declaration validation against `ctx`.
pub fn check_first_party_tools(ctx: &Context) -> Vec<Diagnostic> {
    let mut findings = Vec::new();
    let mut cache: HashMap<String, Option<ResolvedAdapter>> = HashMap::new();
    let mut shape_reported = HashSet::new();

    for expected in EXPECTED_FIRST_PARTY_TOOLS {
        let resolved = cache
            .entry(expected.adapter.to_string())
            .or_insert_with(|| resolve_adapter_declarations(ctx, expected.adapter))
            .clone();

        let Some(resolved) = resolved else {
            continue;
        };

        if shape_reported.insert(expected.adapter.to_string()) {
            findings.extend(resolved.shape_findings);
        }

        let package_request = resolved.declarations.get(expected.name);
        match package_request {
            None => findings.push(invalid_declaration(
                &resolved.rel,
                &resolved.path,
                &format!("missing tool '{}'", expected.name),
            )),
            Some(package) if package != expected.package => findings.push(invalid_declaration(
                &resolved.rel,
                &resolved.path,
                &format!("'{}' package must be '{}'", expected.name, expected.package),
            )),
            _ => {}
        }
    }

    findings
}

#[derive(Clone)]
struct ResolvedAdapter {
    rel: String,
    path: PathBuf,
    declarations: HashMap<String, String>,
    shape_findings: Vec<Diagnostic>,
}

fn resolve_adapter_declarations(ctx: &Context, adapter: &str) -> Option<ResolvedAdapter> {
    let path = ctx.targets_dir().join(adapter).join("adapter.yaml");
    if !path.is_file() {
        return None;
    }

    let rel = path
        .strip_prefix(ctx.framework_root())
        .unwrap_or(&path)
        .to_string_lossy()
        .replace('\\', "/");
    let raw = fs::read_to_string(&path).ok()?;
    let manifest: Value = serde_saphyr::from_str(&raw).ok()?;
    let tools =
        manifest.get("tools").and_then(|value| value.as_array()).cloned().unwrap_or_default();

    let mut shape_findings = Vec::new();
    let mut declarations = HashMap::new();

    for tool in tools {
        let Some(entry) = tool.as_object() else {
            shape_findings.push(invalid_declaration(
                &rel,
                &path,
                "`tools[]` entries must be { name, version } objects under target.schema.json",
            ));
            continue;
        };

        let name = entry.get("name").and_then(|value| value.as_str());
        let version = entry.get("version").and_then(|value| value.as_str());
        let (Some(name), Some(version)) = (name, version) else {
            shape_findings.push(invalid_declaration(
                &rel,
                &path,
                "tool object must carry string `name` and `version` fields",
            ));
            continue;
        };

        declarations.insert(name.to_string(), format!("specify:{name}@{version}"));
    }

    Some(ResolvedAdapter {
        rel,
        path,
        declarations,
        shape_findings,
    })
}

fn invalid_declaration(rel: &str, path: &Path, detail: &str) -> Diagnostic {
    framework_finding(
        RULE_INVALID_DECLARATION,
        format!("First-party tool declaration: {rel} — {detail}"),
        Some(loc(path, 1, None)),
    )
}
