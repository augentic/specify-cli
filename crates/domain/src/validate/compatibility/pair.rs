//! Per-pair contract dispatch — given a producer/consumer contract pair,
//! detect the contract format and route to the appropriate classifier.

use std::path::Path;

use serde_json::Value;

use super::asyncapi_diff::diff_asyncapi;
use super::openapi_diff::diff_openapi;
use super::schema_diff::diff_schema;
use super::{CompatibilityClassification, CompatibilityFinding, repo_contract_path};

pub(super) struct PairContext<'a> {
    pub(super) producer_project: &'a str,
    pub(super) consumer_project: &'a str,
    pub(super) producer_contract: String,
    pub(super) consumer_contract: String,
}

impl PairContext<'_> {
    pub(super) fn finding(
        &self, classification: CompatibilityClassification, change_kind: Option<&str>,
        locator: impl Into<String>, details: impl Into<String>,
    ) -> CompatibilityFinding {
        CompatibilityFinding {
            classification,
            change_kind: change_kind.map(str::to_string),
            producer_project: self.producer_project.to_string(),
            consumer_project: self.consumer_project.to_string(),
            producer_contract: self.producer_contract.clone(),
            consumer_contract: self.consumer_contract.clone(),
            locator: locator.into(),
            details: details.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ContractFormat {
    OpenApi,
    AsyncApi,
    JsonSchema,
}

pub(super) fn diff_pair_contracts(
    project_dir: &Path, rel: &str, ctx: &PairContext<'_>, findings: &mut Vec<CompatibilityFinding>,
) {
    let producer_path = project_dir.join("contracts").join(rel);
    let consumer_path = project_dir
        .join(".specify")
        .join("workspace")
        .join(ctx.consumer_project)
        .join("contracts")
        .join(rel);

    let Some(producer_doc) = read_contract(&producer_path, ctx, true, findings) else {
        return;
    };
    let Some(consumer_doc) = read_contract(&consumer_path, ctx, false, findings) else {
        return;
    };

    let producer_format = detect_format(&producer_doc, rel);
    let consumer_format = detect_format(&consumer_doc, rel);
    let (Some(producer_format), Some(consumer_format)) = (producer_format, consumer_format) else {
        findings.push(ctx.finding(
            CompatibilityClassification::Unverifiable,
            None,
            repo_contract_path(rel),
            "Producer or consumer contract format is unsupported for compatibility classification",
        ));
        return;
    };
    if producer_format != consumer_format {
        findings.push(ctx.finding(
            CompatibilityClassification::Unverifiable,
            None,
            repo_contract_path(rel),
            "Producer and consumer contract files are different formats",
        ));
        return;
    }

    match producer_format {
        ContractFormat::OpenApi => diff_openapi(&consumer_doc, &producer_doc, ctx, findings),
        ContractFormat::AsyncApi => diff_asyncapi(&consumer_doc, &producer_doc, ctx, findings),
        ContractFormat::JsonSchema => {
            diff_schema(&consumer_doc, &producer_doc, ctx, "schema", findings);
        }
    }
}

fn read_contract(
    path: &Path, ctx: &PairContext<'_>, producer: bool, findings: &mut Vec<CompatibilityFinding>,
) -> Option<Value> {
    let content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(err) => {
            let side = if producer { "Producer" } else { "Consumer" };
            findings.push(ctx.finding(
                CompatibilityClassification::Unverifiable,
                None,
                path.display().to_string(),
                format!("{side} contract is missing or unreadable: {err}"),
            ));
            return None;
        }
    };
    match serde_saphyr::from_str::<Value>(&content) {
        Ok(value) => Some(value),
        Err(err) => {
            let side = if producer { "Producer" } else { "Consumer" };
            findings.push(ctx.finding(
                CompatibilityClassification::Unverifiable,
                None,
                path.display().to_string(),
                format!("{side} contract YAML is malformed: {err}"),
            ));
            None
        }
    }
}

fn detect_format(value: &Value, rel: &str) -> Option<ContractFormat> {
    let obj = value.as_object()?;
    if obj.contains_key("openapi") {
        return Some(ContractFormat::OpenApi);
    }
    if obj.contains_key("asyncapi") {
        return Some(ContractFormat::AsyncApi);
    }
    if rel.starts_with("schemas/")
        || obj.contains_key("$schema")
        || obj.contains_key("properties")
        || obj.contains_key("type")
    {
        return Some(ContractFormat::JsonSchema);
    }
    None
}
