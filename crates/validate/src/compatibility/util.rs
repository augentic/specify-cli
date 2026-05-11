//! Shared helpers used across compatibility classifiers.

use serde_json::{Map, Value};

use super::pair::PairContext;
use super::{CompatibilityClassification, CompatibilityFinding};

pub(super) fn empty_object() -> &'static Map<String, Value> {
    static EMPTY: std::sync::OnceLock<Map<String, Value>> = std::sync::OnceLock::new();
    EMPTY.get_or_init(Map::new)
}

#[expect(
    clippy::option_if_let_else,
    reason = "map_or_else cannot express the fallback's static lifetime without escaping the borrowed option"
)]
pub(super) fn object_or_empty(value: Option<&Map<String, Value>>) -> &Map<String, Value> {
    if let Some(object) = value { object } else { empty_object() }
}

pub(super) fn compare_named_objects(
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

pub(super) fn maybe_ambiguous(
    consumer: &Value, producer: &Value, before: usize, locator: &str, ctx: &PairContext<'_>,
    findings: &mut Vec<CompatibilityFinding>,
) {
    if findings.len() == before && semantic_value(consumer) != semantic_value(producer) {
        findings.push(ctx.finding(
            CompatibilityClassification::Ambiguous,
            None,
            locator,
            "Producer and consumer views differ in a construct not classified by the policy table",
        ));
    }
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
