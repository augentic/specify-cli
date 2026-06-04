use super::*;

#[test]
fn github_adapter_uri_parses_default_main() {
    let parsed = GithubAdapterUri::parse("https://github.com/owner/repo/schemas/omnia")
        .expect("parse GitHub URI");
    assert_eq!(
        parsed,
        GithubAdapterUri {
            owner: "owner".to_string(),
            repo: "repo".to_string(),
            checkout_ref: None,
            adapter_path: "schemas/omnia".to_string(),
            adapter_name: "omnia".to_string(),
        }
    );
}

#[test]
fn github_adapter_uri_parses_suffix_ref() {
    let parsed = GithubAdapterUri::parse("https://github.com/owner/repo/schemas/omnia@v1")
        .expect("parse GitHub URI");
    assert_eq!(parsed.checkout_ref.as_deref(), Some("v1"));
    assert_eq!(parsed.adapter_path, "schemas/omnia");
    assert_eq!(parsed.adapter_name, "omnia");
}

#[test]
fn github_adapter_uri_parses_tree_ref() {
    let parsed = GithubAdapterUri::parse("https://github.com/owner/repo/tree/main/schemas/omnia")
        .expect("parse GitHub URI");
    assert_eq!(parsed.checkout_ref.as_deref(), Some("main"));
    assert_eq!(parsed.adapter_path, "schemas/omnia");
    assert_eq!(parsed.adapter_name, "omnia");
}

#[test]
fn name_from_value_handles_shapes() {
    assert_eq!(adapter_name_from_value("omnia"), "omnia");
    assert_eq!(adapter_name_from_value("file:///abs/adapters/targets/omnia"), "omnia");
    assert_eq!(adapter_name_from_value("file:///abs/adapters/targets/omnia/"), "omnia");
    assert_eq!(
        adapter_name_from_value("https://github.com/augentic/specify/adapters/targets/omnia"),
        "omnia"
    );
    assert_eq!(
        adapter_name_from_value("https://github.com/augentic/specify/adapters/targets/omnia@v1"),
        "omnia"
    );
    assert_eq!(adapter_name_from_value("/abs/targets/omnia"), "omnia");
}

#[test]
fn shorthand_splits_name_and_default_ref() {
    assert_eq!(parse_first_party_shorthand("omnia"), Some(("omnia", "v1")));
    assert_eq!(parse_first_party_shorthand("omnia@v2"), Some(("omnia", "v2")));
    assert_eq!(parse_first_party_shorthand("code-typescript"), Some(("code-typescript", "v1")));
    assert_eq!(
        parse_first_party_shorthand("code-typescript@v12"),
        Some(("code-typescript", "v12"))
    );
}

#[test]
fn shorthand_rejects_non_shorthand() {
    // Paths and URLs flow through from_local / from_github instead.
    assert_eq!(parse_first_party_shorthand("./omnia"), None);
    assert_eq!(parse_first_party_shorthand("/abs/omnia"), None);
    assert_eq!(parse_first_party_shorthand("file:///abs/omnia"), None);
    assert_eq!(
        parse_first_party_shorthand("https://github.com/augentic/specify/adapters/targets/omnia"),
        None
    );
    // Not kebab-case, or a non-`vN` ref.
    assert_eq!(parse_first_party_shorthand("Omnia"), None);
    assert_eq!(parse_first_party_shorthand("-omnia"), None);
    assert_eq!(parse_first_party_shorthand("omnia@1"), None);
    assert_eq!(parse_first_party_shorthand("omnia@latest"), None);
    assert_eq!(parse_first_party_shorthand("omnia@"), None);
    assert_eq!(parse_first_party_shorthand(""), None);
}

#[test]
fn shorthand_prefers_framework_root() {
    let root = tempfile::tempdir().expect("tempdir");
    let adapter_dir = root.path().join("adapters").join("targets").join("omnia");
    fs::create_dir_all(&adapter_dir).expect("create adapter dir");
    fs::write(adapter_dir.join(crate::adapter::ADAPTER_FILENAME), "name: omnia\n")
        .expect("write manifest stub");

    let parsed = AdapterUri::from_shorthand("omnia", "v1", Some(root.path()))
        .expect("resolve shorthand against the framework-root checkout");
    assert_eq!(parsed.adapter_name, "omnia");
    assert!(parsed.adapter_value.starts_with("file://"), "{}", parsed.adapter_value);
    assert!(
        parsed.source_dir.ends_with("adapters/targets/omnia"),
        "{}",
        parsed.source_dir.display()
    );
}

#[test]
#[ignore = "networked GitHub fetch smoke test"]
fn shorthand_falls_back_to_github() {
    // No framework root, so the shorthand resolves the canonical
    // published first-party adapter (a real sparse checkout of
    // augentic/specify@v1). Networked — run with `--ignored`.
    let parsed = AdapterUri::from_shorthand("omnia", "v1", None)
        .expect("resolve shorthand against the published GitHub adapter");
    assert_eq!(parsed.adapter_name, "omnia");
}
