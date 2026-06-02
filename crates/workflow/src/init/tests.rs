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
            hub: false,
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
                code: "init-requires-adapter-or-hub",
                ..
            }
        ),
        "got: {err:?}"
    );
}

#[test]
fn hub_init_rejects_adapter_argument() {
    // `--hub` and `<adapter>` are mutually exclusive; the
    // orchestrator re-checks even when the CLI layer already
    // filtered.
    let tmp = tempdir().unwrap();
    let err = init(
        InitOptions {
            project_dir: tmp.path(),
            adapter: Some("omnia"),
            name: Some("platform-hub"),
            description: None,
            hub: true,
            include_framework: false,
            upgrade: false,
        },
        fixed_now(),
    )
    .expect_err("hub + adapter must error");
    assert!(
        matches!(
            &err,
            Error::Diag {
                code: "init-requires-adapter-or-hub",
                ..
            }
        ),
        "got: {err:?}"
    );
}
