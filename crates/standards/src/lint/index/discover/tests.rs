use super::*;

#[test]
fn origin_matches_overlay_precedence() {
    assert_eq!(infer_origin("adapters/shared/rules/universal/UNI-014.md"), Origin::Shared);
    assert_eq!(infer_origin("adapters/targets/omnia/rules/OMNIA-001.md"), Origin::Target,);
    assert_eq!(infer_origin("adapters/sources/documentation/rules/SRC-001.md"), Origin::Source,);
    assert_eq!(infer_origin("organization/local-policy.md"), Origin::Unknown);
}
