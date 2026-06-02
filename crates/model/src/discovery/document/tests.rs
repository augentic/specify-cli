use super::*;

const SAMPLE: &str = "\
# Discovery

Some prose before the inventory.

## Lead inventory

### legacy:user-registration

- lead: user-registration
- source: legacy
- aliases: [account-registration, user-signup]
- synopsis: Registration endpoint accepting email + password.

### legacy:password-reset-request

- lead: password-reset-request
- source: legacy
- aliases: [password-reset]
- synopsis: Reset endpoint.

## Notes

Some trailing prose.
";

#[test]
fn parses_canonical_layout() {
    let doc = Discovery::parse(SAMPLE).expect("parse ok");
    assert_eq!(doc.leads.len(), 2);
    assert_eq!(doc.leads[0].lead, "user-registration");
    assert_eq!(doc.leads[0].source, "legacy");
    assert_eq!(doc.leads[0].aliases.names, vec!["account-registration", "user-signup"]);
    assert_eq!(doc.leads[1].lead, "password-reset-request");
    assert_eq!(doc.leads[1].aliases.names, vec!["password-reset"]);
}

#[test]
fn parse_lead_set_accepts_headingless_blocks() {
    let doc = Discovery::parse_lead_set(
        "\
### user-registration

- lead: user-registration
- aliases: [signup]
- synopsis: Registration endpoint.
",
    )
    .expect("parse ok");

    assert_eq!(doc.leads.len(), 1);
    assert_eq!(doc.leads[0].lead, "user-registration");
    assert_eq!(doc.leads[0].source, "");
    assert_eq!(doc.leads[0].aliases.names, vec!["signup"]);
}

#[test]
fn parse_lead_set_accepts_existing_inventory_heading() {
    let lead_set = "\
## Lead inventory

### user-registration

- lead: user-registration
- synopsis: Registration endpoint.
";
    let framed = Discovery::parse(lead_set).expect("parse ok");
    let lead_set = Discovery::parse_lead_set(lead_set).expect("parse lead set ok");

    assert_eq!(lead_set, framed);
}

#[test]
fn parse_lead_set_accepts_whitespace_only_content() {
    let doc = Discovery::parse_lead_set("\n  \n").expect("parse ok");

    assert!(doc.leads.is_empty());
}

#[test]
fn round_trips_byte_stable_when_unchanged() {
    let doc = Discovery::parse(SAMPLE).expect("parse ok");
    let rendered = doc.render();
    let reparsed = Discovery::parse(&rendered).expect("reparse ok");
    assert_eq!(doc.leads, reparsed.leads);
}

#[test]
fn resolve_lead_matches_id() {
    let doc = Discovery::parse(SAMPLE).expect("parse ok");
    let hit = doc.resolve_lead("user-registration").expect("resolves");
    assert_eq!(hit.lead, "user-registration");
}

#[test]
fn resolve_lead_matches_alias() {
    let doc = Discovery::parse(SAMPLE).expect("parse ok");
    let hit = doc.resolve_lead("password-reset").expect("resolves via alias");
    assert_eq!(hit.lead, "password-reset-request");
}

#[test]
fn resolve_lead_unknown_errors() {
    let doc = Discovery::parse(SAMPLE).expect("parse ok");
    let err = doc.resolve_lead("never-heard-of-it").expect_err("unknown errs");
    match err {
        ResolveError::Unknown { token } => assert_eq!(token, "never-heard-of-it"),
        ResolveError::Collision { .. } => panic!("expected Unknown, got Collision"),
    }
}

#[test]
fn resolve_lead_collision_errors() {
    let yaml = "\
## Lead inventory

### legacy:a

- lead: a
- source: legacy
- aliases: [shared]
- synopsis: A.

### legacy:b

- lead: b
- source: legacy
- aliases: [shared]
- synopsis: B.
";
    let doc = Discovery::parse(yaml).expect("parse ok");
    let err = doc.resolve_lead("shared").expect_err("collision errs");
    match err {
        ResolveError::Collision { token, leads } => {
            assert_eq!(token, "shared");
            assert_eq!(leads, vec!["a".to_string(), "b".to_string()]);
        }
        ResolveError::Unknown { .. } => panic!("expected Collision, got Unknown"),
    }
}

#[test]
fn check_alias_collisions_id_vs_id() {
    // Manually construct a Discovery with a duplicate id (the
    // parser doesn't reject this — the schema check upstream
    // would, but this gate is the cross-check for hand-edited
    // discovery.md files).
    let doc = Discovery {
        prefix: String::new(),
        has_inventory_heading: true,
        suffix: String::new(),
        leads: vec![
            Lead {
                lead: "a".to_string(),
                source: "legacy".to_string(),
                synopsis: "A.".to_string(),
                aliases: LeadAliases::default(),
            },
            Lead {
                lead: "a".to_string(),
                source: "legacy".to_string(),
                synopsis: "Duplicate id.".to_string(),
                aliases: LeadAliases::default(),
            },
        ],
    };
    let findings = doc.check_alias_collisions();
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].source, "legacy");
    assert_eq!(findings[0].name, "a");
    assert_eq!(findings[0].bearing_leads, vec!["a".to_string()]);
}

#[test]
fn same_lead_across_sources_is_legal() {
    // Raw, unmerged leads: the same `lead` surfaced by two
    // different sources is two distinct blocks, not a collision.
    let doc = Discovery {
        prefix: String::new(),
        has_inventory_heading: true,
        suffix: String::new(),
        leads: vec![
            Lead {
                lead: "user-registration".to_string(),
                source: "legacy".to_string(),
                synopsis: "From legacy.".to_string(),
                aliases: LeadAliases::default(),
            },
            Lead {
                lead: "user-registration".to_string(),
                source: "runtime".to_string(),
                synopsis: "From runtime.".to_string(),
                aliases: LeadAliases::default(),
            },
        ],
    };
    assert!(
        doc.check_alias_collisions().is_empty(),
        "same lead under different source keys must not collide"
    );
}

#[test]
fn check_alias_collisions_id_vs_alias() {
    let yaml = "\
## Lead inventory

### legacy:a

- lead: a
- source: legacy
- synopsis: A.

### legacy:b

- lead: b
- source: legacy
- aliases: [a]
- synopsis: B aliases a's id.
";
    let doc = Discovery::parse(yaml).expect("parse ok");
    let findings = doc.check_alias_collisions();
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].name, "a");
    assert_eq!(findings[0].bearing_leads, vec!["a".to_string(), "b".to_string()]);
}

#[test]
fn check_alias_collisions_alias_vs_alias() {
    let yaml = "\
## Lead inventory

### legacy:a

- lead: a
- source: legacy
- aliases: [shared]
- synopsis: A.

### legacy:b

- lead: b
- source: legacy
- aliases: [shared]
- synopsis: B.
";
    let doc = Discovery::parse(yaml).expect("parse ok");
    let findings = doc.check_alias_collisions();
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].name, "shared");
    assert_eq!(findings[0].bearing_leads, vec!["a".to_string(), "b".to_string()]);
}

#[test]
fn alias_collisions_clean_doc() {
    let doc = Discovery::parse(SAMPLE).expect("parse ok");
    let findings = doc.check_alias_collisions();
    assert!(findings.is_empty(), "clean doc must produce no findings; got: {findings:?}");
}

#[test]
fn add_alias_persists_through_render() {
    let mut doc = Discovery::parse(SAMPLE).expect("parse ok");
    doc.add_alias("password-reset-request", "pwd-reset").expect("add ok");
    let rendered = doc.render();
    let reparsed = Discovery::parse(&rendered).expect("reparse ok");
    let lead = reparsed.leads.iter().find(|c| c.lead == "password-reset-request").expect("present");
    assert!(lead.aliases.contains("pwd-reset"));
    assert!(lead.aliases.contains("password-reset"), "preserves existing aliases");
}

#[test]
fn add_alias_refuses_collision() {
    let mut doc = Discovery::parse(SAMPLE).expect("parse ok");
    let err = doc.add_alias("password-reset-request", "user-registration").expect_err("collision");
    match err {
        Error::Validation { code, .. } => {
            assert_eq!(code, "discovery-alias-collision");
        }
        other => panic!("expected Validation, got: {other:?}"),
    }
    // Ensure the mutation rolled back so subsequent edits start
    // from the same state the operator saw on disk.
    let lead = doc.leads.iter().find(|c| c.lead == "password-reset-request").expect("present");
    assert!(!lead.aliases.contains("user-registration"));
}

#[test]
fn add_alias_unknown_lead_errors() {
    let mut doc = Discovery::parse(SAMPLE).expect("parse ok");
    let err = doc.add_alias("nope", "x").expect_err("unknown");
    match err {
        Error::Diag { code, .. } => assert_eq!(code, "discovery-lead-unknown"),
        other => panic!("expected Diag, got: {other:?}"),
    }
}

#[test]
fn remove_alias_idempotent_when_absent() {
    let mut doc = Discovery::parse(SAMPLE).expect("parse ok");
    doc.remove_alias("password-reset-request", "never-set").expect("no-op ok");
    let lead = doc.leads.iter().find(|c| c.lead == "password-reset-request").expect("present");
    assert!(lead.aliases.contains("password-reset"));
}

#[test]
fn remove_alias_drops_named_entry() {
    let mut doc = Discovery::parse(SAMPLE).expect("parse ok");
    doc.remove_alias("password-reset-request", "password-reset").expect("removed");
    let lead = doc.leads.iter().find(|c| c.lead == "password-reset-request").expect("present");
    assert!(!lead.aliases.contains("password-reset"));
}

#[test]
fn parses_block_without_aliases_bullet() {
    let yaml = "\
## Lead inventory

### legacy:a

- lead: a
- source: legacy
- synopsis: A.
";
    let doc = Discovery::parse(yaml).expect("parse ok");
    assert!(doc.leads[0].aliases.is_empty());
}

fn lead(lead: &str, source: &str, synopsis: &str) -> Lead {
    Lead {
        lead: lead.to_string(),
        source: source.to_string(),
        synopsis: synopsis.to_string(),
        aliases: LeadAliases::default(),
    }
}

#[test]
fn merge_survey_replaces_same_id_block() {
    // Re-survey survival — re-running `survey` for a source replaces
    // its leads by canonical `id` in place; untouched leads survive.
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("discovery.md");
    let mut doc = Discovery::parse(SAMPLE).expect("parse ok");

    let incoming =
        vec![lead("user-registration", "legacy", "Registration endpoint (re-surveyed).")];
    doc.merge_survey("legacy", incoming, &path).expect("merge ok");

    let reloaded = Discovery::load(&path).expect("reload ok");
    let hit = reloaded.leads.iter().find(|c| c.lead == "user-registration").expect("present");
    assert_eq!(hit.synopsis, "Registration endpoint (re-surveyed).");
    assert_eq!(
        reloaded.leads.iter().filter(|c| c.lead == "user-registration").count(),
        1,
        "replaced in place, not duplicated"
    );
    assert!(
        reloaded.leads.iter().any(|c| c.lead == "password-reset-request"),
        "leads absent from the incoming set survive untouched"
    );
}

#[test]
fn merge_survey_preserves_operator_aliases() {
    // discovery alias contract §re-survey survival — operator-authored
    // aliases on a surviving id are unioned with the adapter's re-emit.
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("discovery.md");
    let mut doc = Discovery::parse(SAMPLE).expect("parse ok");
    doc.add_alias("password-reset-request", "pwd-reset").expect("operator alias ok");

    // The re-survey re-emits the adapter alias `password-reset`; the
    // operator's `pwd-reset` must survive the union.
    let mut reset = lead("password-reset-request", "legacy", "Reset endpoint (re-surveyed).");
    reset.aliases = LeadAliases::from_iter(["password-reset"]);
    doc.merge_survey("legacy", vec![reset], &path).expect("merge ok");

    let reloaded = Discovery::load(&path).expect("reload ok");
    let hit = reloaded.leads.iter().find(|c| c.lead == "password-reset-request").expect("present");
    assert_eq!(hit.synopsis, "Reset endpoint (re-surveyed).");
    assert_eq!(
        hit.aliases.names,
        vec!["password-reset", "pwd-reset"],
        "operator + adapter aliases union without duplication"
    );
}

#[test]
fn merge_survey_preserves_deterministic_ordering() {
    // Replaced leads keep their document slot; brand-new leads append
    // in survey order, so re-survey re-renders deterministically.
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("discovery.md");
    let doc_md = "\
## Lead inventory

### legacy:x

- lead: x
- source: legacy
- synopsis: X.

### legacy:y

- lead: y
- source: legacy
- synopsis: Y.

### legacy:z

- lead: z
- source: legacy
- synopsis: Z.
";
    let mut doc = Discovery::parse(doc_md).expect("parse ok");

    let incoming = vec![lead("y", "legacy", "Y (re-surveyed)."), lead("w", "legacy", "W (new).")];
    doc.merge_survey("legacy", incoming, &path).expect("merge ok");

    let reloaded = Discovery::load(&path).expect("reload ok");
    let ids: Vec<&str> = reloaded.leads.iter().map(|c| c.lead.as_str()).collect();
    assert_eq!(ids, vec!["x", "y", "z", "w"]);
}

#[test]
fn merge_survey_collision_fails_without_writing() {
    // A post-merge collision fails the whole merge: nothing lands on
    // disk and the in-memory model rolls back to its pre-merge state.
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("discovery.md");
    let mut doc = Discovery::parse(SAMPLE).expect("parse ok");
    let before = doc.clone();

    // Incoming lead aliases another lead's canonical id → collision.
    let mut rogue = lead("new-lead", "legacy", "Rogue.");
    rogue.aliases = LeadAliases::from_iter(["user-registration"]);
    let err = doc.merge_survey("legacy", vec![rogue], &path).expect_err("collision");
    match err {
        Error::Validation { code, .. } => assert_eq!(code, "discovery-alias-collision"),
        other => panic!("expected Validation, got: {other:?}"),
    }

    assert!(!path.exists(), "failed merge must not write the file");
    assert_eq!(doc, before, "failed merge must roll the in-memory model back");
}
