//! Layout-mode integration tests. Exercises the unwired-subset rule
//! and the structural-identity engine.

mod engine_support;

use std::path::PathBuf;

use engine_support::{errors_array, extract_envelope, warnings_array, write_named};
use serde_json::Value;
use vectis_validate::__test_internals::composition_validator;
use vectis_validate::error::VectisError;
use vectis_validate::{Args, ValidateMode, run};

/// Appendix C verbatim. Pinned here as the happy-path schema fixture
/// so any future drift surfaces in this test first. The example
/// exercises the unwired subset end-to-end: regions, groups (one with
/// `component: task-row`), items, token references, asset references,
/// states with the `stateEntry.when` field (which is the bare `when:`
/// -- not a `*-when` key -- and explicitly preserved), overlays
/// without `trigger`, and a `platforms.{ios,android}` block.
const APPENDIX_C_LAYOUT_YAML: &str = r#"version: 1

provenance:
  sources:
    - kind: screenshots
      captured_at: "2026-04-12T10:30:00Z"
    - kind: manual

screens:
  task-list:
    name: Task list
    description: Primary screen showing all open tasks for the signed-in user.
    header:
      title: My tasks
      trailing:
        - icon-button:
            icon: settings
            label: Open settings
    body:
      list:
        each: tasks
        style: plain
        item:
          - group:
              component: task-row
              direction: row
              gap: md
              padding: md
              align: center
              items:
                - checkbox:
                    label: Mark task complete
                - group:
                    direction: column
                    gap: xs
                    size:
                      width: fill
                    items:
                      - text:
                          role: heading
                          style: body
                      - text:
                          style: caption
                          color: on-surface-variant
                - icon:
                    name: chevron-right
                    color: on-surface-variant
    fab:
      icon: plus
      label: Add task
    states:
      empty:
        when: tasks.is_empty
        replaces: body
        body:
          - group:
              direction: column
              gap: md
              padding: lg
              align: center
              justify: center
              items:
                - image:
                    name: empty-tasks-hero
                - text:
                    content: No tasks yet
                    style: title
                - text:
                    content: Tap the + button to add your first task.
                    style: body
                    color: on-surface-variant
      loading:
        when: tasks.is_loading
        replaces: body
        body:
          - progress-indicator:
              style: circular
    overlays:
      delete-confirm:
        kind: dialog
        title: Delete task?
        content:
          - text:
              content: This task will be removed permanently.
          - group:
              direction: row
              gap: sm
              justify: end
              items:
                - button:
                    label: Cancel
                    style: text
                - button:
                    label: Delete
                    style: text
                    color: error

  settings:
    name: Settings
    header:
      title: Settings
      leading:
        - icon-button:
            icon: chevron-left
            label: Back
    body:
      form:
        - group:
            direction: column
            gap: lg
            padding: md
            items:
              - text:
                  content: Appearance
                  role: heading
                  style: title
              - segmented-control:
                  options:
                    - System
                    - Light
                    - Dark
              - text:
                  content: Account
                  role: heading
                  style: title
              - button:
                  label: Sign out
                  style: outlined
                  color: error
    platforms:
      ios:
        header:
          title: Settings
      android:
        header:
          title: Settings
"#;

fn run_layout(content: &str) -> Value {
    let file = write_named(content);
    let args = Args {
        mode: ValidateMode::Layout,
        path: Some(file.path().to_path_buf()),
    };
    extract_envelope(run(&args).expect("run succeeds"))
}

#[test]
fn embedded_composition_schema_compiles() {
    composition_validator().expect("embedded composition.schema.json must compile");
}

/// Acceptance bullet 1: Appendix C's `layout.yaml` validates cleanly.
/// Schema passes (the `screens`-shape `oneOf` branch), no forbidden
/// wiring keys are present, and the single `component: task-row`
/// instance has nothing to compare against -- so structural-identity
/// is a no-op.
#[test]
fn layout_appendix_c_validates_cleanly() {
    let envelope = run_layout(APPENDIX_C_LAYOUT_YAML);
    assert_eq!(envelope["mode"], "layout");
    assert!(errors_array(&envelope).is_empty(), "Appendix C unexpectedly errored: {envelope}");
    assert!(
        warnings_array(&envelope).is_empty(),
        "no warnings expected for Appendix C: {envelope}"
    );
}

/// Acceptance bullet 2: a `bind:` key anywhere in the document
/// produces an error pointing at the offending node.
#[test]
fn layout_bind_key_is_rejected_with_pathful_error() {
    let yaml = r"version: 1
screens:
  s:
    name: S
    body:
      list:
        each: tasks
        item:
          - checkbox:
              bind: tasks.completed
";
    let envelope = run_layout(yaml);
    let errors = errors_array(&envelope);
    let any_hit = errors.iter().any(|e| {
        e["path"].as_str().unwrap_or("").ends_with("/checkbox/bind")
            && e["message"].as_str().unwrap_or("").contains("`bind` is define-owned")
    });
    assert!(any_hit, "expected a `bind` rejection with the offending JSON Pointer: {errors:?}");
}

/// `event:`, `error:`, `maps_to:`, overlay `trigger:`, and a
/// representative `*-when` key (`strikethrough-when`) are all rejected
/// by the unwired-subset walker. The bare `when:` on `stateEntry` --
/// which appears in Appendix C as `when: tasks.is_empty` -- MUST stay
/// allowed; the matrix pinned below also asserts that.
#[test]
fn layout_every_forbidden_wiring_key_is_rejected_but_bare_when_passes() {
    let yaml = r"version: 1
screens:
  s:
    name: S
    maps_to: SomeRoute
    body:
      list:
        each: tasks
        item:
          - text:
              content: hello
              event: Tapped
              error: required
              strikethrough-when: tasks.completed
    overlays:
      sheet:
        kind: sheet
        trigger: OpenSheet
        content:
          - text:
              content: hi
    states:
      empty:
        when: tasks.is_empty
        replaces: body
        body:
          - text:
              content: nothing here
";
    let envelope = run_layout(yaml);
    let errors = errors_array(&envelope);
    let messages: Vec<String> =
        errors.iter().map(|e| e["message"].as_str().unwrap_or("").to_string()).collect();

    for key in [
        "`maps_to` is define-owned",
        "`event` is define-owned",
        "`error` is define-owned",
        "overlay `trigger` is define-owned",
        "`*-when` keys are define-owned",
    ] {
        assert!(
            messages.iter().any(|m| m.contains(key)),
            "expected a finding mentioning {key:?}, got: {messages:?}"
        );
    }

    // The bare `when:` on stateEntry is *not* a forbidden key. No
    // error message should reference `/states/empty/when`.
    assert!(
        !errors.iter().any(|e| e["path"].as_str().unwrap_or("").ends_with("/states/empty/when")),
        "stateEntry.when (bare `when:`) MUST stay allowed: {errors:?}"
    );
}

/// Acceptance bullet 3: a `delta:` document is rejected, even when it
/// would otherwise pass the schema (the schema's `oneOf` permits
/// `delta`). The error points at `/delta`.
#[test]
fn layout_delta_document_is_rejected() {
    let yaml = r"version: 1
delta:
  added:
    new-screen:
      name: New
      body:
        list:
          each: things
          item:
            - text:
                content: hello
";
    let envelope = run_layout(yaml);
    let errors = errors_array(&envelope);
    assert!(
        errors.iter().any(|e| e["path"].as_str().unwrap_or("") == "/delta"
            && e["message"].as_str().unwrap_or("").contains("MUST NOT use the `delta` shape")),
        "expected `/delta` rejection: {errors:?}"
    );
}

/// Acceptance bullet 4 (positive half): two groups in different
/// screens carrying the same `component:` slug with the *same*
/// skeleton but different free text content / token references
/// validate cleanly. The wiring-difference dimension that composition
/// mode cares about (`bind` / `event` / etc.) cannot be exercised in
/// layout mode because those keys are forbidden by the unwired
/// subset; the structural-identity engine still ignores leaf wiring
/// values across all invocations, so the tightest test we can land
/// here exercises content + token-ref divergence with skeleton match.
#[test]
fn layout_same_skeleton_different_wiring_validates_cleanly() {
    let yaml = r"version: 1
screens:
  one:
    name: One
    body:
      - group:
          component: card
          direction: column
          items:
            - text:
                content: First card heading
                style: title
                color: on-surface
            - text:
                content: First card body
                style: body
  two:
    name: Two
    body:
      - group:
          component: card
          direction: column
          items:
            - text:
                content: Second card heading
                style: title
                color: primary
            - text:
                content: Second card body
                style: caption
";
    let envelope = run_layout(yaml);
    assert!(
        errors_array(&envelope).is_empty(),
        "same skeleton + differing leaf values must validate: {envelope}"
    );
}

/// Acceptance bullet 4 (negative half): two groups in different
/// screens carrying the same `component:` slug with materially
/// different skeletons (different ordered nested item kinds) produce a
/// structural-identity error.
#[test]
fn layout_different_skeletons_same_slug_is_an_error() {
    let yaml = r"version: 1
screens:
  one:
    name: One
    body:
      - group:
          component: card
          direction: column
          items:
            - text:
                content: heading
            - text:
                content: body
  two:
    name: Two
    body:
      - group:
          component: card
          direction: column
          items:
            - text:
                content: heading
            - icon:
                name: chevron-right
            - text:
                content: body
";
    let envelope = run_layout(yaml);
    let errors = errors_array(&envelope);
    assert!(
        errors.iter().any(|e| e["message"]
            .as_str()
            .unwrap_or("")
            .contains("component slug `card` has a different skeleton")),
        "expected a structural-identity error for `card`: {errors:?}"
    );
}

/// Edge case: differing nested-group depth between two slug instances
/// also triggers a structural-identity error. This pins the "same
/// nested item kinds, same nesting shape" rule concretely.
#[test]
fn layout_different_nested_group_depth_is_an_error() {
    let yaml = r"version: 1
screens:
  one:
    name: One
    body:
      - group:
          component: row
          direction: row
          items:
            - text:
                content: a
            - text:
                content: b
  two:
    name: Two
    body:
      - group:
          component: row
          direction: row
          items:
            - text:
                content: a
            - group:
                direction: column
                items:
                  - text:
                      content: b
";
    let envelope = run_layout(yaml);
    let errors = errors_array(&envelope);
    assert!(
        errors.iter().any(|e| e["message"]
            .as_str()
            .unwrap_or("")
            .contains("component slug `row` has a different skeleton")),
        "expected a structural-identity error for `row`: {errors:?}"
    );
}

/// Edge case: per-instance `platforms.*` overrides MAY diverge from
/// the base skeleton. The base instances must still match, but a
/// `screens.<n>.platforms.ios.body` instance with a different shape
/// does not trigger the rule.
#[test]
fn layout_platforms_override_instance_is_exempt_from_base_match() {
    let yaml = r"version: 1
screens:
  one:
    name: One
    body:
      - group:
          component: card
          direction: column
          items:
            - text:
                content: heading
    platforms:
      ios:
        body:
          - group:
              component: card
              direction: column
              items:
                - text:
                    content: heading
                - icon:
                    name: chevron-right
                - text:
                    content: body
  two:
    name: Two
    body:
      - group:
          component: card
          direction: column
          items:
            - text:
                content: heading
";
    let envelope = run_layout(yaml);
    assert!(
        errors_array(&envelope).is_empty(),
        "platforms.* override instance MUST be exempt from base-skeleton match: {envelope}"
    );
}

/// A single `component:` instance has nothing to compare against; the
/// structural-identity rule is a no-op until a second base instance
/// appears (matches the conservative emission policy: directives only
/// emitted when ≥2 instances agree on a slug, but the validator does
/// not require that -- it is only sensitive to disagreement).
#[test]
fn layout_single_component_instance_passes_silently() {
    let yaml = r"version: 1
screens:
  one:
    name: One
    body:
      - group:
          component: card
          direction: column
          items:
            - text:
                content: heading
";
    let envelope = run_layout(yaml);
    assert!(
        errors_array(&envelope).is_empty(),
        "single component instance should pass silently: {envelope}"
    );
}

/// Schema rejection still fires for layout-mode (e.g. an unknown
/// screen-property name); the rejection rides the same envelope shape
/// as the unwired-subset / structural-identity errors and the
/// dispatcher exits non-zero.
#[test]
fn layout_schema_violation_reports_pathful_error() {
    let yaml = r"version: 1
screens:
  s:
    name: S
    body:
      list:
        each: tasks
        item:
          - text:
              content: hi
        unknown_listpattern_field: nope
";
    let envelope = run_layout(yaml);
    let errors = errors_array(&envelope);
    assert!(
        !errors.is_empty(),
        "expected at least one schema error for unknown_listpattern_field: {envelope}"
    );
}

/// Reserved component slug (e.g. `header`) is rejected by
/// `composition.schema.json`'s F.2 patch (`component.not.enum`). The
/// layout-mode validator surfaces it as a schema error.
#[test]
fn layout_reserved_component_slug_is_rejected() {
    let yaml = r"version: 1
screens:
  s:
    name: S
    body:
      - group:
          component: header
          direction: column
          items:
            - text:
                content: hi
";
    let envelope = run_layout(yaml);
    let errors = errors_array(&envelope);
    assert!(
        !errors.is_empty(),
        "reserved slug `header` MUST be rejected by the F.2 patch: {envelope}"
    );
}

#[test]
fn layout_invalid_yaml_surfaces_as_a_single_error_entry() {
    let envelope = run_layout(": : not valid yaml :::\n");
    let errors = errors_array(&envelope);
    assert_eq!(errors.len(), 1, "expected one YAML-parse error: {envelope}");
    assert!(
        errors[0]["message"].as_str().unwrap_or("").contains("invalid YAML"),
        "expected `invalid YAML` prefix, got {:?}",
        errors[0]
    );
}

#[test]
fn layout_missing_file_returns_invalid_project_error() {
    let args = Args {
        mode: ValidateMode::Layout,
        path: Some(PathBuf::from("/definitely/not/here/layout.yaml")),
    };
    match run(&args) {
        Err(VectisError::InvalidProject { message }) => {
            assert!(message.contains("layout.yaml not readable"), "unexpected message: {message}");
        }
        other => panic!("expected InvalidProject for missing file, got {other:?}"),
    }
}
