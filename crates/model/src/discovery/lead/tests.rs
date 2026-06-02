use super::*;

#[test]
fn round_trips_minimal_block() {
    let yaml = r"lead: user-registration
source: legacy
synopsis: Registration endpoint accepting email + password.
";
    let parsed: Lead = serde_saphyr::from_str(yaml).expect("parse");
    assert_eq!(parsed.lead, "user-registration");
    assert_eq!(parsed.source, "legacy");
    assert!(parsed.aliases.is_empty(), "missing aliases must default to empty");

    let rendered = serde_saphyr::to_string(&parsed).expect("serialise");
    assert!(!rendered.contains("aliases:"), "empty aliases must elide, got:\n{rendered}");
}

#[test]
fn round_trips_with_aliases() {
    let yaml = r"lead: user-registration
source: legacy
synopsis: Registration endpoint accepting email + password.
aliases:
  - account-registration
  - user-signup
";
    let parsed: Lead = serde_saphyr::from_str(yaml).expect("parse");
    assert_eq!(parsed.aliases.names, vec!["account-registration", "user-signup"]);

    let rendered = serde_saphyr::to_string(&parsed).expect("serialise");
    let reparsed: Lead = serde_saphyr::from_str(&rendered).expect("reparse");
    assert_eq!(parsed, reparsed);
}

#[test]
fn resolves_id_then_aliases() {
    let lead = Lead {
        lead: "user-registration".to_string(),
        source: "legacy".to_string(),
        synopsis: "Registration.".to_string(),
        aliases: LeadAliases::from_iter(["account-registration", "user-signup"]),
    };
    assert!(lead.resolves("user-registration"));
    assert!(lead.resolves("account-registration"));
    assert!(lead.resolves("user-signup"));
    assert!(!lead.resolves("USER-REGISTRATION"), "case-sensitive per discovery alias contract");
    assert!(!lead.resolves("password-reset"));
}

#[test]
fn add_alias_appends_new_value() {
    let mut lead = sample();
    lead.add_alias("account-registration".to_string()).expect("ok");
    assert_eq!(lead.aliases.names, vec!["account-registration"]);
}

#[test]
fn add_alias_idempotent_on_exact_duplicate() {
    let mut lead = sample();
    lead.aliases = LeadAliases::from_iter(["account-registration"]);
    lead.add_alias("account-registration".to_string()).expect("idempotent ok");
    assert_eq!(lead.aliases.names, vec!["account-registration"]);
}

#[test]
fn add_alias_refuses_self_shadow() {
    let mut lead = sample();
    let err = lead.add_alias("user-registration".to_string()).expect_err("self-shadow refused");
    match err {
        AliasCollision::EqualsOwnId { lead, alias } => {
            assert_eq!(lead, "user-registration");
            assert_eq!(alias, "user-registration");
        }
    }
}

#[test]
fn remove_alias_idempotent_when_absent() {
    let mut lead = sample();
    lead.aliases = LeadAliases::from_iter(["x", "y"]);
    lead.remove_alias("z");
    assert_eq!(lead.aliases.names, vec!["x", "y"]);
}

#[test]
fn remove_alias_drops_named_entry() {
    let mut lead = sample();
    lead.aliases = LeadAliases::from_iter(["x", "y", "z"]);
    lead.remove_alias("y");
    assert_eq!(lead.aliases.names, vec!["x", "z"]);
}

fn sample() -> Lead {
    Lead {
        lead: "user-registration".to_string(),
        source: "legacy".to_string(),
        synopsis: "Registration.".to_string(),
        aliases: LeadAliases::default(),
    }
}

#[test]
fn rejects_unknown_fields() {
    let yaml = r"lead: user-registration
source: legacy
synopsis: Registration.
rogue: true
";
    let err =
        serde_saphyr::from_str::<Lead>(yaml).expect_err("deny_unknown_fields must catch rogue");
    assert!(err.to_string().contains("rogue"), "expected error to name rogue, got: {err}");
}
