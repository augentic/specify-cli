//! Integration tests for the `vectis infer` subcommand — deterministic,
//! name-free component clustering over the composition baseline (RFC-40
//! §B2 / Step 5). These tests assert the *mechanism* (clustering, the
//! report shape, the per-cluster evidence) and the structural
//! fingerprint — never that any specific English name emerges, because
//! naming is the build skill's job, not the tool's.

use std::path::{Path, PathBuf};

use serde_json::Value;
use specify_vectis::infer::{InferArgs, run};
use tempfile::TempDir;

/// Write a composition baseline under a fresh tempdir and return the
/// tempdir plus its path.
fn write_baseline(yaml: &str) -> (TempDir, PathBuf) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("composition.yaml");
    std::fs::write(&path, yaml).expect("write composition.yaml");
    (tmp, path)
}

/// Run `infer` against a baseline with the default threshold (2).
fn infer_default(path: &Path) -> Value {
    let args = InferArgs {
        composition: path.to_path_buf(),
        candidate_cache: None,
        parts: None,
        min_occurrences: 2,
    };
    run(&args).expect("infer succeeds")
}

fn clusters(report: &Value) -> &[Value] {
    report.get("clusters").and_then(Value::as_array).expect("clusters array").as_slice()
}

/// A `footer` group repeated verbatim on three screens clusters to a
/// single report entry at `occurrences: 3` carrying all three screen
/// slugs — asserted by fingerprint identity and the cluster shape, never
/// by any English name.
#[test]
fn repeated_group_clusters_across_three_screens() {
    let yaml = r"version: 1
screens:
  home:
    name: Home
    footer:
      - group:
          items:
            - icon-button: { bind: home, event: Navigate(Home) }
            - icon-button: { bind: search, event: Navigate(Search) }
  search:
    name: Search
    footer:
      - group:
          items:
            - icon-button: { bind: home, event: Navigate(Home) }
            - icon-button: { bind: search, event: Navigate(Search) }
  settings:
    name: Settings
    footer:
      - group:
          items:
            - icon-button: { bind: home, event: Navigate(Home) }
            - icon-button: { bind: search, event: Navigate(Search) }
";
    let (_tmp, path) = write_baseline(yaml);
    let report = infer_default(&path);

    assert_eq!(report["version"], 1);
    assert_eq!(report["unmatched_parts"], Value::Array(vec![]));

    let found = clusters(&report);
    assert_eq!(found.len(), 1, "expected exactly one cluster: {report}");
    let cluster = &found[0];

    assert_eq!(cluster["occurrences"], 3);
    assert_eq!(cluster["screens"], serde_json::json!(["home", "search", "settings"]));
    assert!(
        cluster["fingerprint"].as_str().is_some_and(|f| f.len() == 64),
        "fingerprint should be a 64-char sha256 hex string: {cluster}"
    );
    assert_eq!(cluster["bound_slug"], Value::Null);
    assert_eq!(cluster["evidence"]["region"], "footer");
    assert_eq!(cluster["evidence"]["item_kinds"], serde_json::json!(["icon-button"]));
    assert_eq!(
        cluster["evidence"]["event_targets"],
        serde_json::json!(["Navigate(Home)", "Navigate(Search)"])
    );
    assert!(cluster.get("skeleton").is_some(), "cluster carries a normalized skeleton: {cluster}");
}

/// Two structurally distinct repeated groups yield two separate clusters
/// with distinct fingerprints.
#[test]
fn distinct_structures_yield_distinct_clusters() {
    let yaml = r"version: 1
screens:
  home:
    name: Home
    footer:
      - group:
          items:
            - icon-button: {}
            - icon-button: {}
    body:
      - group:
          items:
            - text: {}
            - text: {}
            - text: {}
  search:
    name: Search
    footer:
      - group:
          items:
            - icon-button: {}
            - icon-button: {}
    body:
      - group:
          items:
            - text: {}
            - text: {}
            - text: {}
";
    let (_tmp, path) = write_baseline(yaml);
    let report = infer_default(&path);

    let found = clusters(&report);
    assert_eq!(found.len(), 2, "expected two distinct clusters: {report}");
    let fps: Vec<&str> = found.iter().filter_map(|c| c["fingerprint"].as_str()).collect();
    assert_eq!(fps.len(), 2);
    assert_ne!(fps[0], fps[1], "distinct structures must have distinct fingerprints");
}

/// A group present on only one screen is below the default threshold and
/// is absent from the report.
#[test]
fn single_screen_group_is_below_threshold() {
    let yaml = r"version: 1
screens:
  home:
    name: Home
    footer:
      - group:
          items:
            - icon-button: {}
            - icon-button: {}
  search:
    name: Search
    body:
      - text: { content: hi }
";
    let (_tmp, path) = write_baseline(yaml);
    let report = infer_default(&path);
    assert!(clusters(&report).is_empty(), "single-screen group must not cluster: {report}");
}

/// Wiring variation (different `bind` / `event` values, different icon
/// assets) across screens normalizes away — the groups still cluster as
/// one fingerprint.
#[test]
fn wiring_variation_still_clusters() {
    let yaml = r"version: 1
screens:
  home:
    name: Home
    footer:
      - group:
          items:
            - icon-button: { bind: home, event: Navigate(Home), icon: house }
            - icon-button: { bind: search, event: Navigate(Search), icon: glass }
  profile:
    name: Profile
    footer:
      - group:
          items:
            - icon-button: { bind: inbox, event: Navigate(Inbox), icon: tray }
            - icon-button: { bind: account, event: Navigate(Account), icon: gear }
";
    let (_tmp, path) = write_baseline(yaml);
    let report = infer_default(&path);
    let found = clusters(&report);
    assert_eq!(found.len(), 1, "wiring-only variation must collapse to one cluster: {report}");
    assert_eq!(found[0]["occurrences"], 2);
}

/// The walk descends through `states` and `overlays`, so a `group` inside
/// a state body participates in inference.
#[test]
fn groups_inside_states_participate() {
    let yaml = r"version: 1
screens:
  home:
    name: Home
    states:
      empty:
        when: tasks.is_empty
        body:
          - group:
              items:
                - icon: {}
                - text: {}
  search:
    name: Search
    states:
      empty:
        when: results.is_empty
        body:
          - group:
              items:
                - icon: {}
                - text: {}
";
    let (_tmp, path) = write_baseline(yaml);
    let report = infer_default(&path);
    let found = clusters(&report);
    assert_eq!(found.len(), 1, "group inside a state body must cluster: {report}");
    assert_eq!(found[0]["evidence"]["region"], "states");
}

/// `delta.added` / `delta.modified` screens participate in clustering
/// alongside the `screens:` shape.
#[test]
fn delta_screens_participate() {
    let yaml = r"version: 1
delta:
  added:
    home:
      name: Home
      footer:
        - group:
            items:
              - icon-button: {}
              - icon-button: {}
    search:
      name: Search
      footer:
        - group:
            items:
              - icon-button: {}
              - icon-button: {}
  modified: {}
  removed: {}
";
    let (_tmp, path) = write_baseline(yaml);
    let report = infer_default(&path);
    let found = clusters(&report);
    assert_eq!(found.len(), 1, "delta-added screens must cluster: {report}");
    assert_eq!(found[0]["occurrences"], 2);
}

/// `--min-occurrences 3` excludes a group spanning only two screens.
#[test]
fn higher_threshold_excludes_two_screen_group() {
    let yaml = r"version: 1
screens:
  home:
    name: Home
    footer:
      - group:
          items:
            - icon-button: {}
            - icon-button: {}
  search:
    name: Search
    footer:
      - group:
          items:
            - icon-button: {}
            - icon-button: {}
";
    let (_tmp, path) = write_baseline(yaml);
    let args = InferArgs {
        composition: path.clone(),
        candidate_cache: None,
        parts: None,
        min_occurrences: 3,
    };
    let report = run(&args).expect("infer succeeds");
    assert!(
        report["clusters"].as_array().is_some_and(Vec::is_empty),
        "two-screen group must be excluded at threshold 3: {report}"
    );
}
