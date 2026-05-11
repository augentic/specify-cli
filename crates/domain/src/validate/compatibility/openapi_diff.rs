//! OpenAPI-specific compatibility classification.

use serde_json::Value;

use super::pair::PairContext;
use super::schema_diff::diff_schema;
use super::util::{compare_named_objects, maybe_ambiguous, object_or_empty};
use super::{
    CompatibilityClassification, CompatibilityFinding, KIND_REMOVED_ENDPOINT,
    KIND_STATUS_CODE_REMOVED,
};

pub(super) fn diff_openapi(
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
        if let Some(producer_media) = producer_content.get(media_type)
            && let (Some(consumer_schema), Some(producer_schema)) =
                (consumer_media.get("schema"), producer_media.get("schema"))
        {
            diff_schema(
                consumer_schema,
                producer_schema,
                ctx,
                &format!("paths.{path_name}.{method}.requestBody.content.{media_type}.schema"),
                findings,
            );
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
            if let Some(producer_media) = producer_content.get(media_type)
                && let (Some(consumer_schema), Some(producer_schema)) =
                    (consumer_media.get("schema"), producer_media.get("schema"))
            {
                diff_schema(
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
            diff_schema(
                consumer_schema,
                producer_schema,
                ctx,
                &format!("components.schemas.{name}"),
                findings,
            );
        }
    }
}

const fn http_methods() -> &'static [&'static str] {
    &["get", "put", "post", "delete", "options", "head", "patch", "trace"]
}
