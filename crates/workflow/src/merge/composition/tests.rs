use super::*;

#[test]
fn screens_creates_baseline() {
    let delta =
        "version: 1\nscreens:\n  home:\n    title: Home\n  settings:\n    title: Settings\n";
    let result = merge(None, delta).unwrap();
    assert_eq!(result.output, delta);
    assert_eq!(result.operations, vec![MergeOperation::CreatedBaseline { requirement_count: 2 }]);
}

#[test]
fn delta_adds_screen() {
    let baseline = "version: 1\nscreens:\n  home:\n    title: Home\n";
    let delta = "delta:\n  added:\n    settings:\n      title: Settings\n";
    let result = merge(Some(baseline), delta).unwrap();
    assert!(result.output.contains("settings"));
    assert!(result.output.contains("home"));
    assert_eq!(
        result.operations,
        vec![MergeOperation::Added {
            id: "settings".to_string(),
            name: "settings".to_string(),
        }]
    );
}

#[test]
fn delta_modifies_screen() {
    let baseline = "version: 1\nscreens:\n  home:\n    title: Home\n";
    let delta = "delta:\n  modified:\n    home:\n      title: Home v2\n";
    let result = merge(Some(baseline), delta).unwrap();
    assert!(result.output.contains("Home v2"));
    assert_eq!(
        result.operations,
        vec![MergeOperation::Modified {
            id: "home".to_string(),
            name: "home".to_string(),
        }]
    );
}

#[test]
fn delta_removes_screen() {
    let baseline =
        "version: 1\nscreens:\n  home:\n    title: Home\n  settings:\n    title: Settings\n";
    let delta = "delta:\n  removed:\n    settings:\n      reason: deprecated\n";
    let result = merge(Some(baseline), delta).unwrap();
    assert!(!result.output.contains("settings"));
    assert!(result.output.contains("home"));
    assert_eq!(
        result.operations,
        vec![MergeOperation::Removed {
            id: "settings".to_string(),
            name: "settings".to_string(),
        }]
    );
}

#[test]
fn duplicate_add_errors() {
    let baseline = "version: 1\nscreens:\n  home:\n    title: Home\n";
    let delta = "delta:\n  added:\n    home:\n      title: Another Home\n";
    let err = merge(Some(baseline), delta).unwrap_err();
    match err {
        Error::Diag { code, detail } => {
            assert_eq!(code, "composition-screen-conflict");
            assert!(detail.contains("already exists"));
        }
        other => panic!("expected composition-screen-conflict diag, got {other:?}"),
    }
}

#[test]
fn missing_screen_errors() {
    let baseline = "version: 1\nscreens:\n  home:\n    title: Home\n";
    let delta = "delta:\n  modified:\n    ghost:\n      title: Ghost\n";
    let err = merge(Some(baseline), delta).unwrap_err();
    match err {
        Error::Diag { code, detail } => {
            assert_eq!(code, "composition-screen-conflict");
            assert!(detail.contains("not found"));
        }
        other => panic!("expected composition-screen-conflict diag, got {other:?}"),
    }
}

#[test]
fn missing_screens_and_delta_errors() {
    let delta = "version: 1\nfoo: bar\n";
    let err = merge(None, delta).unwrap_err();
    match err {
        Error::Diag { code, detail } => {
            assert_eq!(code, "composition-delta-empty");
            assert!(detail.contains("neither"));
        }
        other => panic!("expected composition-delta-empty diag, got {other:?}"),
    }
}

#[test]
fn delta_on_empty_baseline() {
    let delta = "delta:\n  added:\n    home:\n      title: Home\n";
    let result = merge(None, delta).unwrap();
    assert!(result.output.contains("home"));
    assert_eq!(
        result.operations,
        vec![MergeOperation::Added {
            id: "home".to_string(),
            name: "home".to_string(),
        }]
    );
}
