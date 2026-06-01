//! runtime capture claim — `kind: example` claim shape (`captures` adapter).
//!
//! Runtime captures join the closed `claimKind` enum as
//! `example`. The body carries `id`, optional `path`, a
//! required `replay-digest: sha256:<hex>`, and the open `input` /
//! `output` JSON-shaped blocks the adapter records from the
//! captured request/response. Bodies larger than 64 `KiB` are stored
//! at `path` with only the digest inline — the cap lives in the
//! adapter brief, not the schema.

use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

/// In-memory shape for a single `kind: example` claim.
///
/// `input` and `output` are deliberately untyped per the schema's
/// open per-kind body posture — the adapter records whatever the
/// captured scenario carried (HTTP method/route/body, message topic /
/// payload shape, scheduled-job arguments). Downstream code consults
/// `replay_digest` for cache fingerprinting and `path` for the
/// on-disk location of the full body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct ExampleClaim {
    /// Closed claim-kind discriminator. Always serialises as the
    /// literal `example`.
    pub kind: ExampleKind,
    /// Stable claim id (required on `example` per the schema's `allOf`
    /// branch). Resolves the same id space the provenance table joins
    /// against.
    pub id: String,
    /// Optional `<path>#L<n>` anchor (the capture's on-disk location).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// `sha256:<hex>` digest of the capture bytes. The cache layer
    /// (extraction cache fingerprint contract) keys against this value.
    pub replay_digest: String,
    /// Optional inline input payload — typically the captured request.
    /// Shape is open per the schema's per-kind body posture.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<JsonValue>,
    /// Optional inline output payload — typically the captured
    /// response plus side-effects. Shape is open.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<JsonValue>,
    /// Optional single-line statement describing the capture's
    /// behavioural meaning (the line synthesis lifts into `spec.md`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub statement: Option<String>,
}

/// Single-variant marker enum locking the `kind:` discriminator to
/// the literal `example` on the wire.
///
/// The variant is the *only* legal value for an [`ExampleClaim`];
/// generic deserialisation through `Claim<ExampleClaim>` would
/// otherwise accept any string. Sibling per-kind claim shapes (when
/// they land) follow the same pattern with their own marker enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, strum::Display)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum ExampleKind {
    /// `kind: example`.
    Example,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn fixture() -> ExampleClaim {
        ExampleClaim {
            kind: ExampleKind::Example,
            id: "users.register.happy-path".to_string(),
            path: Some("tests/data/replays/users-register/happy.json".to_string()),
            replay_digest: "sha256:7a2b".to_string(),
            input: Some(json!({
                "method": "POST",
                "route": "/users",
            })),
            output: Some(json!({
                "status": 201,
            })),
            statement: Some(
                "Registering with a fresh email returns 201 and publishes user.created."
                    .to_string(),
            ),
        }
    }

    #[test]
    fn round_trips_full_body() {
        let claim = fixture();
        let yaml = serde_saphyr::to_string(&claim).expect("serialise");
        assert!(yaml.contains("kind: example"));
        assert!(yaml.contains("id: users.register.happy-path"));
        assert!(yaml.contains("replay-digest:"));
        let reparsed: ExampleClaim = serde_saphyr::from_str(&yaml).expect("reparse");
        assert_eq!(claim, reparsed);
    }

    #[test]
    fn elides_optional_fields_when_absent() {
        let claim = ExampleClaim {
            kind: ExampleKind::Example,
            id: "users.register.minimal".to_string(),
            path: None,
            replay_digest: "sha256:deadbeef".to_string(),
            input: None,
            output: None,
            statement: None,
        };
        let yaml = serde_saphyr::to_string(&claim).expect("serialise");
        for absent in ["path:", "input:", "output:", "statement:"] {
            assert!(!yaml.contains(absent), "{absent} must elide when None, got:\n{yaml}");
        }
    }

    #[test]
    fn rejects_unknown_fields() {
        let raw = r#"{
            "kind": "example",
            "id": "x",
            "replay-digest": "sha256:a",
            "rogue": true
        }"#;
        let err =
            serde_json::from_str::<ExampleClaim>(raw).expect_err("unknown field must be rejected");
        assert!(
            err.to_string().contains("unknown field"),
            "expected deny_unknown_fields error, got: {err}"
        );
    }
}
