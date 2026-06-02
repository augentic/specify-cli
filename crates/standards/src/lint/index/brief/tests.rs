use super::*;
use crate::lint::FileKind;

fn brief(relative: &str, body: &str) -> DiscoveredFile {
    DiscoveredFile {
        relative: relative.into(),
        kind: FileKind::Text,
        language: Some("markdown".into()),
        bytes: Some(body.as_bytes().to_vec()),
    }
}

#[test]
fn captures_h2_sections_in_order() {
    let file = brief(
        "adapters/sources/intent/briefs/survey.md",
        "# Title\n\n## Inputs\n\nbody\n\n## Output contract\n\nmore body\n",
    );
    let brief = extract(&file).expect("brief extracted");
    assert_eq!(brief.axis, AdapterAxis::Sources);
    assert_eq!(brief.adapter, "intent");
    assert_eq!(brief.operation, "survey");
    assert_eq!(brief.sections, vec!["Inputs", "Output contract"]);
    assert!(brief.body_line_count >= 3);
}

#[test]
fn rejects_paths_outside_briefs_dir() {
    let file = brief("adapters/sources/intent/references/foo.md", "## Heading\n");
    assert!(extract(&file).is_none());
}
