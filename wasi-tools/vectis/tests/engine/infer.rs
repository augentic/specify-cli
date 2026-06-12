//! Integration tests for the `vectis infer` subcommand — deterministic,
//! name-free component clustering over the composition baseline.
//! These tests assert the *mechanism* (clustering, the
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

/// Run `infer` against a baseline plus a candidate cache at the default
/// threshold (2).
fn infer_with_cache(composition: &Path, cache: &Path) -> Value {
    let args = InferArgs {
        composition: composition.to_path_buf(),
        candidate_cache: Some(cache.to_path_buf()),
        parts: None,
        min_occurrences: 2,
    };
    run(&args).expect("infer succeeds")
}

/// Run `infer` against a baseline plus an operator parts file at the
/// default threshold (2).
fn infer_with_parts(composition: &Path, parts: &Path) -> Value {
    let args = InferArgs {
        composition: composition.to_path_buf(),
        candidate_cache: None,
        parts: Some(parts.to_path_buf()),
        min_occurrences: 2,
    };
    run(&args).expect("infer succeeds")
}

/// Write an operator parts file under a fresh tempdir and return its path.
fn write_parts(tmp: &Path, yaml: &str) -> PathBuf {
    let path = tmp.join("parts.yaml");
    std::fs::write(&path, yaml).expect("write parts.yaml");
    path
}

/// Write a candidate-cache entry at the §B4 provenance path
/// `<slice>/<screen>/<group-path>.yaml` under `cache_root`.
fn write_cache_entry(cache_root: &Path, slice: &str, screen: &str, group_path: &str, body: &str) {
    let dir = cache_root.join(slice).join(screen);
    std::fs::create_dir_all(&dir).expect("create cache dir");
    std::fs::write(dir.join(format!("{group_path}.yaml")), body).expect("write cache entry");
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
    assert_eq!(report["unmatched-parts"], Value::Array(vec![]));

    let found = clusters(&report);
    assert_eq!(found.len(), 1, "expected exactly one cluster: {report}");
    let cluster = &found[0];

    assert_eq!(cluster["occurrences"], 3);
    assert_eq!(cluster["screens"], serde_json::json!(["home", "search", "settings"]));
    assert!(
        cluster["fingerprint"].as_str().is_some_and(|f| f.len() == 64),
        "fingerprint should be a 64-char sha256 hex string: {cluster}"
    );
    assert_eq!(cluster["bound-slug"], Value::Null);
    assert_eq!(cluster["evidence"]["region"], "footer");
    assert_eq!(cluster["evidence"]["item-kinds"], serde_json::json!(["icon-button"]));
    assert_eq!(
        cluster["evidence"]["event-targets"],
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

/// A cached candidate skeleton plus one structurally identical baseline
/// group on a different screen cluster to a single candidate at the
/// default threshold (2): the cache supplies cross-slice memory before
/// the baseline accumulates the second screen.
#[test]
fn cached_skeleton_clusters_with_baseline_group() {
    let baseline = r"version: 1
screens:
  home:
    name: Home
    footer:
      - group:
          items:
            - icon-button: { bind: home, event: Navigate(Home) }
            - icon-button: { bind: search, event: Navigate(Search) }
";
    let (tmp, composition) = write_baseline(baseline);
    let cache = tmp.path().join("cache");
    write_cache_entry(
        &cache,
        "checkout-slice",
        "checkout",
        "footer.0",
        "candidate_component: nav-footer
region: footer
group:
  items:
    - icon-button: { bind: home, event: Navigate(Home) }
    - icon-button: { bind: orders, event: Navigate(Orders) }
",
    );

    let report = infer_with_cache(&composition, &cache);
    let found = clusters(&report);
    assert_eq!(found.len(), 1, "cache + baseline group must cluster as one candidate: {report}");
    let cluster = &found[0];
    assert_eq!(cluster["occurrences"], 2);
    assert_eq!(
        cluster["screens"],
        serde_json::json!(["checkout", "home"]),
        "the cache provenance screen and the baseline screen are distinct screens"
    );
    assert_eq!(
        cluster["evidence"]["candidate-names"],
        serde_json::json!(["nav-footer"]),
        "the cache label hint surfaces as non-authoritative evidence"
    );
    assert_eq!(cluster["bound-slug"], Value::Null, "the tool still invents no name");
}

/// Two cached candidates on distinct provenance screens cluster on their
/// own at the default threshold even with no matching baseline group —
/// the cross-slice-memory case where the baseline has not yet accumulated
/// either screen.
#[test]
fn cache_only_candidates_cluster_across_screens() {
    let baseline = r"version: 1
screens:
  home:
    name: Home
    body:
      - text: { content: hi }
";
    let (tmp, composition) = write_baseline(baseline);
    let cache = tmp.path().join("cache");
    let entry = "group:
  items:
    - icon: {}
    - text: {}
";
    write_cache_entry(&cache, "slice-a", "alpha", "body.0", entry);
    write_cache_entry(&cache, "slice-b", "beta", "body.0", entry);

    let report = infer_with_cache(&composition, &cache);
    let found = clusters(&report);
    assert_eq!(found.len(), 1, "two cached screens must cluster: {report}");
    assert_eq!(found[0]["occurrences"], 2);
    assert_eq!(found[0]["screens"], serde_json::json!(["alpha", "beta"]));
}

/// Two cache entries for the *same* provenance screen count once — the
/// distinct-screen rule applies to cached candidates too, so they stay
/// below the default threshold.
#[test]
fn cache_entries_same_screen_count_once() {
    let baseline = r"version: 1
screens:
  home:
    name: Home
    body:
      - text: { content: hi }
";
    let (tmp, composition) = write_baseline(baseline);
    let cache = tmp.path().join("cache");
    let entry = "group:
  items:
    - icon: {}
    - text: {}
";
    write_cache_entry(&cache, "slice-a", "alpha", "body.0", entry);
    write_cache_entry(&cache, "slice-a", "alpha", "body.1", entry);

    let report = infer_with_cache(&composition, &cache);
    assert!(
        clusters(&report).is_empty(),
        "two groups on one cached screen count once, below threshold 2: {report}"
    );
}

/// A malformed cache entry (no `group` fragment) and a non-YAML file are
/// skipped — inference is best-effort and never aborts on cache noise.
#[test]
fn malformed_cache_entries_are_skipped() {
    let baseline = r"version: 1
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
    let (tmp, composition) = write_baseline(baseline);
    let cache = tmp.path().join("cache");
    write_cache_entry(&cache, "slice-a", "alpha", "noise", "not: a group entry\n");
    std::fs::write(cache.join("slice-a").join("alpha").join("readme.txt"), "ignore me")
        .expect("write txt");

    let report = infer_with_cache(&composition, &cache);
    let found = clusters(&report);
    assert_eq!(found.len(), 1, "baseline cluster survives, cache noise ignored: {report}");
    assert_eq!(found[0]["occurrences"], 2);
}

/// A pinned operator part matching a single baseline group is promoted
/// to a cluster below the default threshold (promotion authority) and
/// carries the operator slug in `bound-slug` plus `pinned: true` (naming
/// authority). The tool still derives no name of its own.
#[test]
fn pinned_part_promotes_below_threshold() {
    let baseline = r"version: 1
screens:
  home:
    name: Home
    footer:
      - group:
          items:
            - icon-button: { bind: home, event: Navigate(Home) }
            - icon-button: { bind: search, event: Navigate(Search) }
";
    let (tmp, composition) = write_baseline(baseline);
    let parts = write_parts(
        tmp.path(),
        r"version: 1
parts:
  primary-nav:
    description: Operator-defined nav bar.
    group:
      items:
        - icon-button: { bind: a, event: Navigate(A) }
        - icon-button: { bind: b, event: Navigate(B) }
",
    );

    let report = infer_with_parts(&composition, &parts);
    let found = clusters(&report);
    assert_eq!(found.len(), 1, "the single matched group is promoted by the pin: {report}");
    let cluster = &found[0];
    assert_eq!(cluster["occurrences"], 1);
    assert_eq!(cluster["bound-slug"], "primary-nav", "the operator slug is echoed");
    assert_eq!(cluster["pinned"], Value::Bool(true));
    assert_eq!(report["unmatched-parts"], serde_json::json!([]));
}

/// A pinned part whose skeleton matches no baseline group is not a
/// cluster — it is surfaced under `unmatched-parts`, and inference
/// proceeds regardless.
#[test]
fn pinned_part_matching_nothing_is_unmatched() {
    let baseline = r"version: 1
screens:
  home:
    name: Home
    body:
      - text: { content: hi }
";
    let (tmp, composition) = write_baseline(baseline);
    let parts = write_parts(
        tmp.path(),
        r"version: 1
parts:
  primary-nav:
    group:
      items:
        - icon-button: {}
        - icon-button: {}
        - icon-button: {}
",
    );

    let report = infer_with_parts(&composition, &parts);
    assert!(clusters(&report).is_empty(), "no baseline group matches the pin: {report}");
    assert_eq!(report["unmatched-parts"], serde_json::json!(["primary-nav"]));
}

/// When a baseline group matches no pin, the pin still surfaces under
/// `unmatched-parts` while the group clusters only on its own merits
/// (here below threshold, so absent) — pins never suppress ordinary
/// clustering.
#[test]
fn unpinned_group_and_unmatched_pin_coexist() {
    let baseline = r"version: 1
screens:
  home:
    name: Home
    footer:
      - group:
          items:
            - icon-button: {}
            - icon-button: {}
";
    let (tmp, composition) = write_baseline(baseline);
    let parts = write_parts(
        tmp.path(),
        r"version: 1
parts:
  hero:
    group:
      items:
        - image: {}
        - text: {}
        - text: {}
",
    );

    let report = infer_with_parts(&composition, &parts);
    assert!(clusters(&report).is_empty(), "the lone footer group is below threshold: {report}");
    assert_eq!(report["unmatched-parts"], serde_json::json!(["hero"]));
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
