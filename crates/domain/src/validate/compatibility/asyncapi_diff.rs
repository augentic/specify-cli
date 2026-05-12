//! AsyncAPI-specific compatibility classification.

use serde_json::Value;

use super::pair::PairContext;
use super::schema_diff::diff_schema;
use super::util::{compare_named_objects, maybe_ambiguous, object_or_empty};
use super::{CompatibilityFinding, KIND_REMOVED_CHANNEL, KIND_REMOVED_OPERATION};

pub(super) fn diff_asyncapi(
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

fn compare_asyncapi_payloads(
    consumer: &Value, producer: &Value, ctx: &PairContext<'_>,
    findings: &mut Vec<CompatibilityFinding>,
) {
    let consumer_messages =
        object_or_empty(consumer.pointer("/components/messages").and_then(Value::as_object));
    let producer_messages =
        object_or_empty(producer.pointer("/components/messages").and_then(Value::as_object));
    for (name, consumer_message) in consumer_messages {
        if let Some(producer_message) = producer_messages.get(name)
            && let (Some(consumer_payload), Some(producer_payload)) =
                (consumer_message.get("payload"), producer_message.get("payload"))
        {
            diff_schema(
                consumer_payload,
                producer_payload,
                ctx,
                &format!("components.messages.{name}.payload"),
                findings,
            );
        }
    }
}
