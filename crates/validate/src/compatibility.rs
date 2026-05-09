//! Cross-project contract compatibility classification.
//!
//! The classifier compares producer contracts in the current project
//! baseline with consumer views materialised under `.specify/workspace/`.
//! It is deliberately read-only and deterministic: callers provide a
//! project root, the module reads `registry.yaml`, `contracts/`, and
//! workspace clones, then returns a classified report.

use std::collections::BTreeSet;
use std::path::Path;

use serde::Serialize;
use serde_json::{Map, Value};
use specify_error::Error;
use specify_registry::{Registry, RegistryProject};

use crate::contracts::validate_baseline_contracts;

const KIND_REMOVED_FIELD: &str = "removed-field";
const KIND_REQUIRED_FIELD_ADDED: &str = "required-field-added";
const KIND_TYPE_NARROWED: &str = "type-narrowed";
const KIND_ENUM_VALUE_REMOVED: &str = "enum-value-removed";
const KIND_ADDITIONAL_PROPERTIES_TIGHTENED: &str = "additional-properties-tightened";
const KIND_REMOVED_ENDPOINT: &str = "removed-endpoint";
const KIND_STATUS_CODE_REMOVED: &str = "status-code-removed";
const KIND_REMOVED_CHANNEL: &str = "removed-channel";
const KIND_REMOVED_OPERATION: &str = "removed-operation";

/// RM-04 compatibility buckets for a producer-to-consumer contract delta.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CompatibilityClassification {
    /// The delta is backwards-compatible for the consumer.
    Additive,
    /// The delta is a recognized backwards-incompatible wire change.
    Breaking,
    /// The delta changed behaviorally, but the classifier cannot prove
    /// whether it is safe.
    Ambiguous,
    /// The classifier could not compare the producer and consumer views.
    Unverifiable,
}

impl CompatibilityClassification {
    fn as_str(self) -> &'static str {
        match self {
            Self::Additive => "additive",
            Self::Breaking => "breaking",
            Self::Ambiguous => "ambiguous",
            Self::Unverifiable => "unverifiable",
        }
    }
}

/// One compatibility finding for a producer / consumer contract pair.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct CompatibilityFinding {
    /// RM-04 classification bucket.
    pub classification: CompatibilityClassification,
    /// Shared `change-kind` value when the delta maps to the contract
    /// vocabulary.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub change_kind: Option<String>,
    /// Registry project that produces the contract.
    pub producer_project: String,
    /// Registry project that consumes the contract.
    pub consumer_project: String,
    /// Contract path from the producer baseline, relative to repo root.
    pub producer_contract: String,
    /// Contract path from the consumer workspace, relative to repo root.
    pub consumer_contract: String,
    /// Human-readable locator inside the contract document.
    pub locator: String,
    /// Human-readable diagnostic detail.
    pub details: String,
}

/// Aggregate counts for a compatibility report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct CompatibilitySummary {
    /// Number of findings in the report.
    pub total_findings: usize,
    /// Number of `additive` findings.
    pub additive: usize,
    /// Number of `breaking` findings.
    pub breaking: usize,
    /// Number of `ambiguous` findings.
    pub ambiguous: usize,
    /// Number of `unverifiable` findings.
    pub unverifiable: usize,
}

impl CompatibilitySummary {
    fn from_findings(findings: &[CompatibilityFinding]) -> Self {
        let mut summary = Self {
            total_findings: findings.len(),
            additive: 0,
            breaking: 0,
            ambiguous: 0,
            unverifiable: 0,
        };
        for finding in findings {
            match finding.classification {
                CompatibilityClassification::Additive => summary.additive += 1,
                CompatibilityClassification::Breaking => summary.breaking += 1,
                CompatibilityClassification::Ambiguous => summary.ambiguous += 1,
                CompatibilityClassification::Unverifiable => summary.unverifiable += 1,
            }
        }
        summary
    }
}

/// Full cross-project compatibility report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct CompatibilityReport {
    /// Optional change name supplied by the CLI.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub change: Option<String>,
    /// Number of producer / consumer contract pairs inspected.
    pub checked_pairs: usize,
    /// `true` when the report has no blocking compatibility risk.
    pub ok: bool,
    /// Per-delta findings.
    pub findings: Vec<CompatibilityFinding>,
    /// Aggregate counts by classification.
    pub summary: CompatibilitySummary,
}

impl CompatibilityReport {
    fn new(
        change: Option<String>, checked_pairs: usize, mut findings: Vec<CompatibilityFinding>,
    ) -> Self {
        findings.sort_by(|a, b| {
            a.producer_project
                .cmp(&b.producer_project)
                .then_with(|| a.consumer_project.cmp(&b.consumer_project))
                .then_with(|| a.producer_contract.cmp(&b.producer_contract))
                .then_with(|| a.locator.cmp(&b.locator))
                .then_with(|| a.classification.as_str().cmp(b.classification.as_str()))
        });
        let summary = CompatibilitySummary::from_findings(&findings);
        let ok = summary.breaking == 0 && summary.ambiguous == 0 && summary.unverifiable == 0;
        Self {
            change,
            checked_pairs,
            ok,
            findings,
            summary,
        }
    }

    /// Returns `true` when the report contains only clean or additive
    /// compatibility results.
    #[must_use]
    pub const fn is_compatible(&self) -> bool {
        self.ok
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ContractFormat {
    OpenApi,
    AsyncApi,
    JsonSchema,
}

struct PairContext<'a> {
    producer_project: &'a str,
    consumer_project: &'a str,
    producer_contract: String,
    consumer_contract: String,
}

impl PairContext<'_> {
    fn finding(
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

/// Classify cross-project compatibility for the current project.
///
/// # Errors
///
/// Returns an error when `registry.yaml` cannot be loaded.
pub fn classify_project_compatibility(
    project_dir: &Path, change: Option<String>,
) -> Result<CompatibilityReport, Error> {
    let Some(registry) = Registry::load(project_dir)? else {
        return Ok(CompatibilityReport::new(change, 0, Vec::new()));
    };
    let baseline_findings = validate_baseline_contracts(&project_dir.join("contracts"));
    let mut findings = Vec::new();
    let mut checked_pairs = 0;

    for producer in &registry.projects {
        let Some(roles) = &producer.contracts else {
            continue;
        };
        for produced in &roles.produces {
            let rel = normalize_contract_path(produced);
            for consumer in consumers_for(&registry, producer, &rel) {
                checked_pairs += 1;
                let ctx = PairContext {
                    producer_project: &producer.name,
                    consumer_project: &consumer.name,
                    producer_contract: repo_contract_path(&rel),
                    consumer_contract: repo_contract_path(&rel),
                };
                classify_pair(project_dir, &rel, &baseline_findings, &ctx, &mut findings);
            }
        }
    }

    Ok(CompatibilityReport::new(change, checked_pairs, findings))
}

fn consumers_for<'a>(
    registry: &'a Registry, producer: &RegistryProject, rel: &str,
) -> Vec<&'a RegistryProject> {
    registry
        .projects
        .iter()
        .filter(|candidate| candidate.name != producer.name)
        .filter(|candidate| {
            candidate.contracts.as_ref().is_some_and(|roles| {
                roles.consumes.iter().any(|consumed| normalize_contract_path(consumed) == rel)
            })
        })
        .collect()
}

fn classify_pair(
    project_dir: &Path, rel: &str, baseline_findings: &[crate::contracts::ContractFinding],
    ctx: &PairContext<'_>, findings: &mut Vec<CompatibilityFinding>,
) {
    let pair_baseline_findings: Vec<_> =
        baseline_findings.iter().filter(|finding| finding.path.ends_with(rel)).collect();
    if !pair_baseline_findings.is_empty() {
        for finding in pair_baseline_findings {
            findings.push(ctx.finding(
                CompatibilityClassification::Unverifiable,
                None,
                repo_contract_path(rel),
                format!("Producer baseline failed `{}`: {}", finding.rule_id, finding.detail),
            ));
        }
        return;
    }

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
        ContractFormat::OpenApi => classify_openapi(&consumer_doc, &producer_doc, ctx, findings),
        ContractFormat::AsyncApi => classify_asyncapi(&consumer_doc, &producer_doc, ctx, findings),
        ContractFormat::JsonSchema => {
            classify_schema(&consumer_doc, &producer_doc, ctx, "schema", findings);
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

fn classify_openapi(
    consumer: &Value, producer: &Value, ctx: &PairContext<'_>,
    findings: &mut Vec<CompatibilityFinding>,
) {
    let before = findings.len();
    compare_named_objects(
        consumer.pointer("/paths").and_then(Value::as_object),
        producer.pointer("/paths").and_then(Value::as_object),
        "paths",
        KIND_REMOVED_ENDPOINT,
        "endpoint",
        ctx,
        findings,
    );
    compare_openapi_operations(consumer, producer, ctx, findings);
    compare_component_schemas(consumer, producer, ctx, findings);
    maybe_ambiguous(consumer, producer, before, "openapi", ctx, findings);
}

fn classify_asyncapi(
    consumer: &Value, producer: &Value, ctx: &PairContext<'_>,
    findings: &mut Vec<CompatibilityFinding>,
) {
    let before = findings.len();
    compare_named_objects(
        consumer.pointer("/channels").and_then(Value::as_object),
        producer.pointer("/channels").and_then(Value::as_object),
        "channels",
        KIND_REMOVED_CHANNEL,
        "channel",
        ctx,
        findings,
    );
    compare_named_objects(
        consumer.pointer("/operations").and_then(Value::as_object),
        producer.pointer("/operations").and_then(Value::as_object),
        "operations",
        KIND_REMOVED_OPERATION,
        "operation",
        ctx,
        findings,
    );
    compare_asyncapi_payloads(consumer, producer, ctx, findings);
    maybe_ambiguous(consumer, producer, before, "asyncapi", ctx, findings);
}

fn compare_named_objects(
    consumer: Option<&Map<String, Value>>, producer: Option<&Map<String, Value>>, locator: &str,
    removed_kind: &str, noun: &str, ctx: &PairContext<'_>,
    findings: &mut Vec<CompatibilityFinding>,
) {
    let consumer = object_or_empty(consumer);
    let producer = object_or_empty(producer);
    for name in consumer.keys() {
        if !producer.contains_key(name) {
            findings.push(ctx.finding(
                CompatibilityClassification::Breaking,
                Some(removed_kind),
                format!("{locator}.{name}"),
                format!(
                    "Consumer view defines {noun} `{name}`, but the producer contract removed it"
                ),
            ));
        }
    }
    for name in producer.keys() {
        if !consumer.contains_key(name) {
            findings.push(ctx.finding(
                CompatibilityClassification::Additive,
                None,
                format!("{locator}.{name}"),
                format!("Producer contract adds {noun} `{name}`"),
            ));
        }
    }
}

fn compare_openapi_operations(
    consumer: &Value, producer: &Value, ctx: &PairContext<'_>,
    findings: &mut Vec<CompatibilityFinding>,
) {
    let consumer_paths = object_or_empty(consumer.pointer("/paths").and_then(Value::as_object));
    let producer_paths = object_or_empty(producer.pointer("/paths").and_then(Value::as_object));
    for (path_name, consumer_path) in consumer_paths {
        let Some(producer_path) = producer_paths.get(path_name) else {
            continue;
        };
        let consumer_ops = object_or_empty(consumer_path.as_object());
        let producer_ops = object_or_empty(producer_path.as_object());
        for method in http_methods().iter().copied() {
            match (consumer_ops.get(method), producer_ops.get(method)) {
                (Some(_), None) => findings.push(ctx.finding(
                    CompatibilityClassification::Breaking,
                    Some(KIND_REMOVED_ENDPOINT),
                    format!("paths.{path_name}.{method}"),
                    format!("Consumer view defines `{method} {path_name}`, but the producer contract removed it"),
                )),
                (None, Some(_)) => findings.push(ctx.finding(
                    CompatibilityClassification::Additive,
                    None,
                    format!("paths.{path_name}.{method}"),
                    format!("Producer contract adds `{method} {path_name}`"),
                )),
                (Some(consumer_op), Some(producer_op)) => {
                    compare_openapi_operation(path_name, method, consumer_op, producer_op, ctx, findings);
                }
                (None, None) => {}
            }
        }
    }
}

fn compare_openapi_operation(
    path_name: &str, method: &str, consumer: &Value, producer: &Value, ctx: &PairContext<'_>,
    findings: &mut Vec<CompatibilityFinding>,
) {
    compare_openapi_request_body(path_name, method, consumer, producer, ctx, findings);
    compare_openapi_responses(path_name, method, consumer, producer, ctx, findings);
}

fn compare_openapi_request_body(
    path_name: &str, method: &str, consumer: &Value, producer: &Value, ctx: &PairContext<'_>,
    findings: &mut Vec<CompatibilityFinding>,
) {
    let consumer_content =
        object_or_empty(consumer.pointer("/requestBody/content").and_then(Value::as_object));
    let producer_content =
        object_or_empty(producer.pointer("/requestBody/content").and_then(Value::as_object));
    for (media_type, consumer_media) in consumer_content {
        if let Some(producer_media) = producer_content.get(media_type) {
            if let (Some(consumer_schema), Some(producer_schema)) =
                (consumer_media.get("schema"), producer_media.get("schema"))
            {
                classify_schema(
                    consumer_schema,
                    producer_schema,
                    ctx,
                    &format!("paths.{path_name}.{method}.requestBody.content.{media_type}.schema"),
                    findings,
                );
            }
        }
    }
}

fn compare_openapi_responses(
    path_name: &str, method: &str, consumer: &Value, producer: &Value, ctx: &PairContext<'_>,
    findings: &mut Vec<CompatibilityFinding>,
) {
    let consumer_responses =
        object_or_empty(consumer.pointer("/responses").and_then(Value::as_object));
    let producer_responses =
        object_or_empty(producer.pointer("/responses").and_then(Value::as_object));
    for (status, consumer_response) in consumer_responses {
        let Some(producer_response) = producer_responses.get(status) else {
            findings.push(ctx.finding(
                CompatibilityClassification::Breaking,
                Some(KIND_STATUS_CODE_REMOVED),
                format!("paths.{path_name}.{method}.responses.{status}"),
                format!("Consumer view defines response `{status}` for `{method} {path_name}`, but the producer contract removed it"),
            ));
            continue;
        };
        let consumer_content =
            object_or_empty(consumer_response.pointer("/content").and_then(Value::as_object));
        let producer_content =
            object_or_empty(producer_response.pointer("/content").and_then(Value::as_object));
        for (media_type, consumer_media) in consumer_content {
            if let Some(producer_media) = producer_content.get(media_type) {
                if let (Some(consumer_schema), Some(producer_schema)) =
                    (consumer_media.get("schema"), producer_media.get("schema"))
                {
                    classify_schema(
                        consumer_schema,
                        producer_schema,
                        ctx,
                        &format!(
                            "paths.{path_name}.{method}.responses.{status}.content.{media_type}.schema"
                        ),
                        findings,
                    );
                }
            }
        }
    }
}

fn compare_component_schemas(
    consumer: &Value, producer: &Value, ctx: &PairContext<'_>,
    findings: &mut Vec<CompatibilityFinding>,
) {
    let consumer_schemas =
        object_or_empty(consumer.pointer("/components/schemas").and_then(Value::as_object));
    let producer_schemas =
        object_or_empty(producer.pointer("/components/schemas").and_then(Value::as_object));
    for (name, consumer_schema) in consumer_schemas {
        if let Some(producer_schema) = producer_schemas.get(name) {
            classify_schema(
                consumer_schema,
                producer_schema,
                ctx,
                &format!("components.schemas.{name}"),
                findings,
            );
        }
    }
}

fn compare_asyncapi_payloads(
    consumer: &Value, producer: &Value, ctx: &PairContext<'_>,
    findings: &mut Vec<CompatibilityFinding>,
) {
    let consumer_messages =
        object_or_empty(consumer.pointer("/components/messages").and_then(Value::as_object));
    let producer_messages =
        object_or_empty(producer.pointer("/components/messages").and_then(Value::as_object));
    for (name, consumer_message) in consumer_messages {
        if let Some(producer_message) = producer_messages.get(name) {
            if let (Some(consumer_payload), Some(producer_payload)) =
                (consumer_message.get("payload"), producer_message.get("payload"))
            {
                classify_schema(
                    consumer_payload,
                    producer_payload,
                    ctx,
                    &format!("components.messages.{name}.payload"),
                    findings,
                );
            }
        }
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "The schema diff is intentionally local so the policy table is easy to audit."
)]
fn classify_schema(
    consumer: &Value, producer: &Value, ctx: &PairContext<'_>, locator: &str,
    findings: &mut Vec<CompatibilityFinding>,
) {
    let before = findings.len();
    if consumer.get("$ref").is_some() || producer.get("$ref").is_some() {
        if consumer != producer {
            findings.push(ctx.finding(
                CompatibilityClassification::Ambiguous,
                None,
                locator,
                "Schema reference changed; classifier does not resolve referenced documents in RM-04",
            ));
        }
        return;
    }

    let consumer_obj = object_or_empty(consumer.as_object());
    let producer_obj = object_or_empty(producer.as_object());
    let consumer_required = required_set(consumer);
    let producer_required = required_set(producer);

    for field in producer_required.difference(&consumer_required) {
        findings.push(ctx.finding(
            CompatibilityClassification::Breaking,
            Some(KIND_REQUIRED_FIELD_ADDED),
            format!("{locator}.required"),
            format!("Producer contract adds required field `{field}`"),
        ));
    }

    compare_type(consumer, producer, ctx, locator, findings);
    compare_enum(consumer, producer, ctx, locator, findings);
    compare_constraints(consumer_obj, producer_obj, ctx, locator, findings);
    compare_additional_properties(consumer, producer, ctx, locator, findings);

    let consumer_properties =
        object_or_empty(consumer.get("properties").and_then(Value::as_object));
    let producer_properties =
        object_or_empty(producer.get("properties").and_then(Value::as_object));
    for (name, consumer_property) in consumer_properties {
        match producer_properties.get(name) {
            Some(producer_property) => classify_schema(
                consumer_property,
                producer_property,
                ctx,
                &format!("{locator}.properties.{name}"),
                findings,
            ),
            None => findings.push(ctx.finding(
                CompatibilityClassification::Breaking,
                Some(KIND_REMOVED_FIELD),
                format!("{locator}.properties.{name}"),
                format!(
                    "Consumer view defines property `{name}`, but the producer contract removed it"
                ),
            )),
        }
    }
    for name in producer_properties.keys() {
        if consumer_properties.contains_key(name) {
            continue;
        }
        let classification = if producer_required.contains(name) {
            CompatibilityClassification::Breaking
        } else {
            CompatibilityClassification::Additive
        };
        let kind = if classification == CompatibilityClassification::Breaking {
            Some(KIND_REQUIRED_FIELD_ADDED)
        } else {
            None
        };
        findings.push(ctx.finding(
            classification,
            kind,
            format!("{locator}.properties.{name}"),
            format!("Producer contract adds property `{name}`"),
        ));
    }

    maybe_ambiguous(consumer, producer, before, locator, ctx, findings);
}

fn compare_type(
    consumer: &Value, producer: &Value, ctx: &PairContext<'_>, locator: &str,
    findings: &mut Vec<CompatibilityFinding>,
) {
    let consumer_types = type_set(consumer);
    let producer_types = type_set(producer);
    if consumer_types.is_empty() || producer_types.is_empty() || consumer_types == producer_types {
        return;
    }
    if producer_types.is_subset(&consumer_types) {
        findings.push(ctx.finding(
            CompatibilityClassification::Breaking,
            Some(KIND_TYPE_NARROWED),
            format!("{locator}.type"),
            "Producer contract narrows the accepted JSON type set",
        ));
    } else if consumer_types.is_subset(&producer_types) {
        findings.push(ctx.finding(
            CompatibilityClassification::Additive,
            None,
            format!("{locator}.type"),
            "Producer contract widens the accepted JSON type set",
        ));
    } else {
        findings.push(ctx.finding(
            CompatibilityClassification::Ambiguous,
            None,
            format!("{locator}.type"),
            "Producer contract changes the JSON type set in a way the classifier cannot order",
        ));
    }
}

fn compare_enum(
    consumer: &Value, producer: &Value, ctx: &PairContext<'_>, locator: &str,
    findings: &mut Vec<CompatibilityFinding>,
) {
    let consumer_enum = value_set(consumer.get("enum"));
    let producer_enum = value_set(producer.get("enum"));
    if consumer_enum.is_empty() || producer_enum.is_empty() || consumer_enum == producer_enum {
        return;
    }
    if !consumer_enum.is_subset(&producer_enum) {
        findings.push(ctx.finding(
            CompatibilityClassification::Breaking,
            Some(KIND_ENUM_VALUE_REMOVED),
            format!("{locator}.enum"),
            "Producer contract removes one or more enum values from the consumer view",
        ));
    } else if producer_enum.len() > consumer_enum.len() {
        findings.push(ctx.finding(
            CompatibilityClassification::Additive,
            None,
            format!("{locator}.enum"),
            "Producer contract adds enum values",
        ));
    }
}

fn compare_constraints(
    consumer: &Map<String, Value>, producer: &Map<String, Value>, ctx: &PairContext<'_>,
    locator: &str, findings: &mut Vec<CompatibilityFinding>,
) {
    for key in [
        "format",
        "pattern",
        "minimum",
        "exclusiveMinimum",
        "maximum",
        "exclusiveMaximum",
        "minLength",
        "maxLength",
        "minItems",
        "maxItems",
    ] {
        if producer.get(key).is_some() && consumer.get(key) != producer.get(key) {
            findings.push(ctx.finding(
                CompatibilityClassification::Breaking,
                Some(KIND_TYPE_NARROWED),
                format!("{locator}.{key}"),
                format!("Producer contract changes constraint `{key}`"),
            ));
        }
    }
}

fn compare_additional_properties(
    consumer: &Value, producer: &Value, ctx: &PairContext<'_>, locator: &str,
    findings: &mut Vec<CompatibilityFinding>,
) {
    let consumer_false =
        consumer.get("additionalProperties").and_then(Value::as_bool) == Some(false);
    let producer_false =
        producer.get("additionalProperties").and_then(Value::as_bool) == Some(false);
    if !consumer_false && producer_false {
        findings.push(ctx.finding(
            CompatibilityClassification::Breaking,
            Some(KIND_ADDITIONAL_PROPERTIES_TIGHTENED),
            format!("{locator}.additionalProperties"),
            "Producer contract disallows additional properties that the consumer view allowed",
        ));
    } else if consumer_false && !producer_false {
        findings.push(ctx.finding(
            CompatibilityClassification::Additive,
            None,
            format!("{locator}.additionalProperties"),
            "Producer contract loosens additionalProperties",
        ));
    }
}

fn maybe_ambiguous(
    consumer: &Value, producer: &Value, before: usize, locator: &str, ctx: &PairContext<'_>,
    findings: &mut Vec<CompatibilityFinding>,
) {
    if findings.len() == before && semantic_value(consumer) != semantic_value(producer) {
        findings.push(ctx.finding(
            CompatibilityClassification::Ambiguous,
            None,
            locator,
            "Producer and consumer views differ in a construct not classified by RM-04",
        ));
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

fn normalize_contract_path(path: &str) -> String {
    path.strip_prefix("contracts/").unwrap_or(path).trim_start_matches('/').to_string()
}

fn repo_contract_path(rel: &str) -> String {
    format!("contracts/{rel}")
}

fn required_set(schema: &Value) -> BTreeSet<String> {
    schema
        .get("required")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect()
}

fn type_set(schema: &Value) -> BTreeSet<String> {
    match schema.get("type") {
        Some(Value::String(s)) => BTreeSet::from([s.clone()]),
        Some(Value::Array(items)) => {
            items.iter().filter_map(Value::as_str).map(str::to_string).collect()
        }
        _ => BTreeSet::new(),
    }
}

fn value_set(value: Option<&Value>) -> BTreeSet<String> {
    value.and_then(Value::as_array).into_iter().flatten().map(Value::to_string).collect()
}

fn semantic_value(value: &Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.iter().map(semantic_value).collect()),
        Value::Object(map) => {
            let mut out = Map::new();
            for (key, item) in map {
                if metadata_key(key) {
                    continue;
                }
                out.insert(key.clone(), semantic_value(item));
            }
            Value::Object(out)
        }
        other => other.clone(),
    }
}

fn metadata_key(key: &str) -> bool {
    matches!(
        key,
        "title"
            | "description"
            | "summary"
            | "example"
            | "examples"
            | "externalDocs"
            | "$comment"
            | "info"
            | "tags"
    )
}

fn empty_object() -> &'static Map<String, Value> {
    static EMPTY: std::sync::OnceLock<Map<String, Value>> = std::sync::OnceLock::new();
    EMPTY.get_or_init(Map::new)
}

fn object_or_empty(value: Option<&Map<String, Value>>) -> &Map<String, Value> {
    match value {
        Some(object) => object,
        None => empty_object(),
    }
}

fn http_methods() -> &'static [&'static str] {
    &["get", "put", "post", "delete", "options", "head", "patch", "trace"]
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::*;

    fn finding_classes(findings: &[CompatibilityFinding]) -> Vec<CompatibilityClassification> {
        findings.iter().map(|finding| finding.classification).collect()
    }

    fn ctx() -> PairContext<'static> {
        PairContext {
            producer_project: "backend",
            consumer_project: "mobile",
            producer_contract: "contracts/schemas/user.yaml".to_string(),
            consumer_contract: "contracts/schemas/user.yaml".to_string(),
        }
    }

    fn yaml(value: &str) -> Value {
        serde_saphyr::from_str(value).expect("test YAML parses")
    }

    #[test]
    fn optional_field_added_is_additive() {
        let old = yaml("type: object\nproperties:\n  id:\n    type: string\nrequired: [id]\n");
        let new = yaml(
            "type: object\nproperties:\n  id:\n    type: string\n  nickname:\n    type: string\nrequired: [id]\n",
        );
        let mut findings = Vec::new();
        classify_schema(&old, &new, &ctx(), "schema", &mut findings);
        assert_eq!(finding_classes(&findings), vec![CompatibilityClassification::Additive]);
    }

    #[test]
    fn required_field_added_is_breaking() {
        let old = yaml("type: object\nproperties:\n  id:\n    type: string\nrequired: [id]\n");
        let new = yaml(
            "type: object\nproperties:\n  id:\n    type: string\n  phone:\n    type: string\nrequired: [id, phone]\n",
        );
        let mut findings = Vec::new();
        classify_schema(&old, &new, &ctx(), "schema", &mut findings);
        assert!(findings.iter().any(|finding| {
            finding.classification == CompatibilityClassification::Breaking
                && finding.change_kind.as_deref() == Some(KIND_REQUIRED_FIELD_ADDED)
        }));
    }

    #[test]
    fn removed_field_is_breaking() {
        let old = yaml(
            "type: object\nproperties:\n  id:\n    type: string\n  email:\n    type: string\n",
        );
        let new = yaml("type: object\nproperties:\n  id:\n    type: string\n");
        let mut findings = Vec::new();
        classify_schema(&old, &new, &ctx(), "schema", &mut findings);
        assert_eq!(findings[0].change_kind.as_deref(), Some(KIND_REMOVED_FIELD));
    }

    #[test]
    fn narrowed_enum_is_breaking() {
        let old = yaml("type: string\nenum: [apple, google]\n");
        let new = yaml("type: string\nenum: [apple]\n");
        let mut findings = Vec::new();
        classify_schema(&old, &new, &ctx(), "schema", &mut findings);
        assert_eq!(findings[0].change_kind.as_deref(), Some(KIND_ENUM_VALUE_REMOVED));
    }

    #[test]
    fn unsupported_changed_construct_is_ambiguous() {
        let old = yaml("type: object\noneOf:\n  - type: string\n");
        let new = yaml("type: object\noneOf:\n  - type: integer\n");
        let mut findings = Vec::new();
        classify_schema(&old, &new, &ctx(), "schema", &mut findings);
        assert_eq!(finding_classes(&findings), vec![CompatibilityClassification::Ambiguous]);
    }

    #[test]
    fn missing_consumer_baseline_is_unverifiable() {
        let tmp = TempDir::new().expect("tempdir");
        write_project(&tmp);
        write_file(
            &tmp.path().join("contracts/schemas/user.yaml"),
            "type: object\nproperties:\n  id:\n    type: string\n",
        );
        let report =
            classify_project_compatibility(tmp.path(), Some("demo".to_string())).expect("report");
        assert_eq!(report.checked_pairs, 1);
        assert_eq!(report.summary.unverifiable, 1);
        assert!(!report.is_compatible());
    }

    fn write_project(tmp: &TempDir) {
        write_file(&tmp.path().join(".specify/project.yaml"), "name: hub\nhub: true\nrules: {}\n");
        write_file(
            &tmp.path().join("registry.yaml"),
            "version: 1\nprojects:\n  - name: backend\n    url: ../backend\n    schema: omnia@v1\n    description: Backend service.\n    contracts:\n      produces:\n        - schemas/user.yaml\n  - name: mobile\n    url: ../mobile\n    schema: vectis@v1\n    description: Mobile client.\n    contracts:\n      consumes:\n        - schemas/user.yaml\n",
        );
    }

    fn write_file(path: &Path, content: &str) {
        fs::create_dir_all(path.parent().expect("test path has parent")).expect("mkdir");
        fs::write(path, content).expect("write file");
    }
}
