//! Schema-level compatibility classification.

use std::collections::BTreeSet;

use serde_json::{Map, Value};

use super::pair::PairContext;
use super::util::{maybe_ambiguous, object_or_empty};
use super::{
    CompatibilityClassification, CompatibilityFinding, KIND_ADDITIONAL_PROPERTIES_TIGHTENED,
    KIND_ENUM_VALUE_REMOVED, KIND_REMOVED_FIELD, KIND_REQUIRED_FIELD_ADDED, KIND_TYPE_NARROWED,
};

pub(super) fn diff_schema(
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
                "Schema reference changed; classifier does not resolve referenced documents",
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
            Some(producer_property) => diff_schema(
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
        let kind = (classification == CompatibilityClassification::Breaking)
            .then_some(KIND_REQUIRED_FIELD_ADDED);
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

#[cfg(test)]
mod tests {
    use super::super::pair::PairContext;
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
        diff_schema(&old, &new, &ctx(), "schema", &mut findings);
        assert_eq!(finding_classes(&findings), vec![CompatibilityClassification::Additive]);
    }

    #[test]
    fn required_field_added_is_breaking() {
        let old = yaml("type: object\nproperties:\n  id:\n    type: string\nrequired: [id]\n");
        let new = yaml(
            "type: object\nproperties:\n  id:\n    type: string\n  phone:\n    type: string\nrequired: [id, phone]\n",
        );
        let mut findings = Vec::new();
        diff_schema(&old, &new, &ctx(), "schema", &mut findings);
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
        diff_schema(&old, &new, &ctx(), "schema", &mut findings);
        assert_eq!(findings[0].change_kind.as_deref(), Some(KIND_REMOVED_FIELD));
    }

    #[test]
    fn narrowed_enum_is_breaking() {
        let old = yaml("type: string\nenum: [apple, google]\n");
        let new = yaml("type: string\nenum: [apple]\n");
        let mut findings = Vec::new();
        diff_schema(&old, &new, &ctx(), "schema", &mut findings);
        assert_eq!(findings[0].change_kind.as_deref(), Some(KIND_ENUM_VALUE_REMOVED));
    }

    #[test]
    fn unsupported_changed_construct_is_ambiguous() {
        let old = yaml("type: object\noneOf:\n  - type: string\n");
        let new = yaml("type: object\noneOf:\n  - type: integer\n");
        let mut findings = Vec::new();
        diff_schema(&old, &new, &ctx(), "schema", &mut findings);
        assert_eq!(finding_classes(&findings), vec![CompatibilityClassification::Ambiguous]);
    }
}
