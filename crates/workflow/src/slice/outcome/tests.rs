use super::*;

#[test]
fn outcome_display_matches_serde() {
    assert_eq!(Kind::Success.to_string(), "success");
    assert_eq!(Kind::Failure.to_string(), "failure");
    assert_eq!(Kind::Deferred.to_string(), "deferred");
}
