use super::Lead;

#[test]
fn round_trips_minimal_lead() {
    let yaml = r"
lead: user-registration
source: legacy-monolith
synopsis: Registration endpoint accepting email + password.
";
    let parsed: Lead = serde_saphyr::from_str(yaml).expect("parse");
    assert_eq!(parsed.lead, "user-registration");
    assert_eq!(parsed.source, "legacy-monolith");
    let rendered = serde_saphyr::to_string(&parsed).expect("render");
    assert!(rendered.contains("user-registration"));
}

#[test]
fn serde_rejects_retired_aliases_field() {
    let yaml = r"
lead: user-registration
source: legacy-monolith
synopsis: Registration endpoint.
aliases:
  - account-registration
";
    let err = serde_saphyr::from_str::<Lead>(yaml).expect_err("aliases field must fail");
    let msg = err.to_string();
    assert!(msg.contains("unknown field") || msg.contains("aliases"), "{msg}");
}
