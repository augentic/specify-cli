use super::*;

#[test]
fn namespace_prefix_extracts_ids() {
    assert_eq!(namespace_prefix("CORE-009"), Some("CORE"));
    assert_eq!(namespace_prefix("UNI-014"), Some("UNI"));
    assert_eq!(namespace_prefix("OMNIA-001"), Some("OMNIA"));
    assert_eq!(namespace_prefix("bad"), None);
    assert_eq!(namespace_prefix("core-009"), None);
    assert_eq!(namespace_prefix("CORE-9"), None);
}

#[test]
fn owned_namespaces_maps_known_directories() {
    assert_eq!(
        owned_namespaces("adapters/shared/rules/core/CORE-009-x.md"),
        Some(BTreeSet::from(["CORE"])),
    );
    assert_eq!(
        owned_namespaces("adapters/shared/rules/universal/UNI-001.md"),
        Some(BTreeSet::from(["UNI"])),
    );
    assert_eq!(
        owned_namespaces("adapters/targets/omnia/rules/OMNIA-001.md"),
        Some(BTreeSet::from(["OMNIA", "RUST", "SEC"])),
    );
    assert_eq!(
        owned_namespaces("adapters/sources/documentation/rules/SRC-001.md"),
        Some(BTreeSet::from(["SRC"])),
    );
    assert_eq!(owned_namespaces("docs/standards/style.md"), None);
}
