use super::*;

#[test]
fn load_returns_none_when_absent() {
    let dir = tempfile::tempdir().expect("tempdir");
    let result = ComponentsCatalog::load(dir.path()).expect("no error");
    assert!(result.is_none());
}

#[test]
fn confirmed_slugs_filters_correctly() {
    let catalog = ComponentsCatalog {
        version: 1,
        components: BTreeMap::from([
            (
                "tab-bar".to_string(),
                ComponentEntry {
                    status: ComponentStatus::Confirmed,
                    description: None,
                },
            ),
            (
                "card-row".to_string(),
                ComponentEntry {
                    status: ComponentStatus::Confirmed,
                    description: None,
                },
            ),
            (
                "hero-banner".to_string(),
                ComponentEntry {
                    status: ComponentStatus::Rejected,
                    description: None,
                },
            ),
        ]),
    };
    let mut slugs = catalog.confirmed_slugs();
    slugs.sort_unstable();
    assert_eq!(slugs, vec!["card-row", "tab-bar"]);
}

#[test]
fn rejected_slugs_filters_correctly() {
    let catalog = ComponentsCatalog {
        version: 1,
        components: BTreeMap::from([
            (
                "tab-bar".to_string(),
                ComponentEntry {
                    status: ComponentStatus::Confirmed,
                    description: None,
                },
            ),
            (
                "hero-banner".to_string(),
                ComponentEntry {
                    status: ComponentStatus::Rejected,
                    description: None,
                },
            ),
        ]),
    };
    assert_eq!(catalog.rejected_slugs(), vec!["hero-banner"]);
}

#[test]
fn status_of_returns_correct_variant() {
    let catalog = ComponentsCatalog {
        version: 1,
        components: BTreeMap::from([(
            "tab-bar".to_string(),
            ComponentEntry {
                status: ComponentStatus::Confirmed,
                description: Some("Bottom nav".to_string()),
            },
        )]),
    };
    assert_eq!(catalog.status_of("tab-bar"), Some(ComponentStatus::Confirmed));
    assert_eq!(catalog.status_of("missing"), None);
}

#[test]
fn round_trip_yaml() {
    let yaml = "version: 1\ncomponents:\n  tab-bar:\n    status: confirmed\n    description: \"Bottom navigation\"\n  hero-banner:\n    status: rejected\n";
    let path = Path::new("test.yaml");
    let catalog = ComponentsCatalog::from_yaml(yaml, path).expect("valid");
    assert_eq!(catalog.version, 1);
    assert_eq!(catalog.components.len(), 2);
    assert_eq!(catalog.status_of("tab-bar"), Some(ComponentStatus::Confirmed));
    assert_eq!(catalog.status_of("hero-banner"), Some(ComponentStatus::Rejected));
}

#[test]
fn rejects_missing_version() {
    let yaml = "components:\n  tab-bar:\n    status: confirmed\n";
    let path = Path::new("test.yaml");
    ComponentsCatalog::from_yaml(yaml, path).unwrap_err();
}

#[test]
fn rejects_invalid_status() {
    let yaml = "version: 1\ncomponents:\n  tab-bar:\n    status: pending\n";
    let path = Path::new("test.yaml");
    ComponentsCatalog::from_yaml(yaml, path).unwrap_err();
}

#[test]
fn rejects_non_kebab_slug() {
    let yaml = "version: 1\ncomponents:\n  TabBar:\n    status: confirmed\n";
    let path = Path::new("test.yaml");
    ComponentsCatalog::from_yaml(yaml, path).unwrap_err();
}

#[test]
fn empty_components_is_valid() {
    let yaml = "version: 1\ncomponents: {}\n";
    let path = Path::new("test.yaml");
    let catalog = ComponentsCatalog::from_yaml(yaml, path).expect("valid");
    assert!(catalog.components.is_empty());
    assert!(catalog.confirmed_slugs().is_empty());
}
