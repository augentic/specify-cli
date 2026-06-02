use super::*;

#[test]
fn round_trips_through_yaml_with_empties_elided() {
    let lock = TopologyLock::from_projects(vec![
        TopologyProject {
            name: "identity-contracts".to_string(),
            target: "contracts@v1".to_string(),
            description: Some("Contracts crate.".to_string()),
            capabilities: vec!["contracts".to_string()],
            keywords: Vec::new(),
        },
        TopologyProject {
            name: "identity-service".to_string(),
            target: "omnia@v1".to_string(),
            description: None,
            capabilities: Vec::new(),
            keywords: Vec::new(),
        },
    ]);

    let yaml = serde_saphyr::to_string(&lock).expect("serialize lock");
    assert!(yaml.contains("name: identity-contracts"), "{yaml}");
    assert!(!yaml.contains("keywords:"), "empty keywords elided: {yaml}");

    let parsed: TopologyLock = serde_saphyr::from_str(&yaml).expect("round-trip");
    assert_eq!(parsed, lock);
}

#[test]
fn save_then_load_is_identity() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("topology.lock");
    let lock = TopologyLock::from_projects(vec![TopologyProject {
        name: "svc".to_string(),
        target: "omnia@v1".to_string(),
        description: None,
        capabilities: Vec::new(),
        keywords: Vec::new(),
    }]);

    lock.save(&path).expect("save");
    let loaded = TopologyLock::load(&path).expect("load").expect("present");
    assert_eq!(loaded, lock);
}

#[test]
fn missing_file_is_none() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("absent.lock");
    assert_eq!(TopologyLock::load(&path).expect("load"), None);
}

#[test]
fn version_too_new_errors() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("topology.lock");
    fs::write(&path, "version: 99\nprojects: []\n").expect("write");
    let err = TopologyLock::load(&path).expect_err("too new");
    assert!(format!("{err:?}").contains("topology-lock-version-too-new"), "{err:?}");
}
