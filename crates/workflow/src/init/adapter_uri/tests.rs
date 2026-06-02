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
