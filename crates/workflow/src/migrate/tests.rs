use super::*;

#[test]
fn resolve_empty_when_to_equals_from() {
    assert!(MigrationKind::resolve(2, 2).is_empty());
    assert!(MigrationKind::resolve(0, 0).is_empty());
}

#[test]
fn resolve_empty_when_to_below_from() {
    assert!(MigrationKind::resolve(2, 1).is_empty());
}

#[test]
fn resolve_dormant_without_migrators() {
    // No major-version migrators are registered, so every window
    // resolves to an empty chain regardless of the span.
    assert!(MigrationKind::resolve(0, 1).is_empty());
    assert!(MigrationKind::resolve(1, 2).is_empty());
    assert!(MigrationKind::resolve(1, 3).is_empty());
}

#[test]
fn major_parses_or_none() {
    assert_eq!(major("1.2.3"), Some(1));
    assert_eq!(major("2.0.0"), Some(2));
    assert_eq!(major("not-a-semver"), None);
}
