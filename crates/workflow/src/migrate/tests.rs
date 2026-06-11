use super::*;

#[test]
fn resolve_dormant_without_migrators() {
    // No major-version migrators are registered, so every window —
    // equal, backwards, or spanning — resolves to an empty chain.
    for (from, to) in [(2, 2), (0, 0), (2, 1), (0, 1), (1, 2), (1, 3)] {
        assert!(MigrationKind::resolve(from, to).is_empty(), "({from}, {to}) should be dormant");
    }
}

#[test]
fn major_parses_or_none() {
    assert_eq!(major("1.2.3"), Some(1));
    assert_eq!(major("2.0.0"), Some(2));
    assert_eq!(major("not-a-semver"), None);
}
