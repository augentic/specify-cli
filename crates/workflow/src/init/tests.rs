use tempfile::tempdir;

use super::*;

#[test]
fn regular_init_rejects_missing_adapter() {
    let tmp = tempdir().unwrap();
    let err = init(
        InitOptions {
            project_dir: tmp.path(),
            adapter: None,
            name: Some("demo"),
            description: None,
            workspace: false,
            include_framework: false,
            upgrade: false,
        },
        fixed_now(),
    )
    .expect_err("missing adapter must error");
    assert!(
        matches!(
            &err,
            Error::Diag {
                code: "init-requires-adapter-or-workspace",
                ..
            }
        ),
        "got: {err:?}"
    );
}

#[test]
fn workspace_init_rejects_adapter_argument() {
    // `--workspace` and `<adapter>` are mutually exclusive; the
    // orchestrator re-checks even when the CLI layer already
    // filtered.
    let tmp = tempdir().unwrap();
    let err = init(
        InitOptions {
            project_dir: tmp.path(),
            adapter: Some("omnia"),
            name: Some("platform-workspace"),
            description: None,
            workspace: true,
            include_framework: false,
            upgrade: false,
        },
        fixed_now(),
    )
    .expect_err("workspace + adapter must error");
    assert!(
        matches!(
            &err,
            Error::Diag {
                code: "init-requires-adapter-or-workspace",
                ..
            }
        ),
        "got: {err:?}"
    );
}
