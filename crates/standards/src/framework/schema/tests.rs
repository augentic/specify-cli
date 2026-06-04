use std::io::Write;

use super::*;

#[test]
fn validate_rejects_invalid_description() {
    let mut temp = tempfile::NamedTempFile::new().expect("temp file");
    write!(temp, "---\nname: spec-test-skill\ndescription: Too short.\n---\n")
        .expect("write temp frontmatter");

    let result = validate_frontmatter(temp.path(), SchemaId::Skill);
    let SchemaError::Validation(errors) = result.expect_err("invalid frontmatter should fail")
    else {
        panic!("expected validation errors");
    };
    assert!(
        errors.iter().any(|error| {
            error.instance_path.contains("description") || error.message.contains("Use when")
        }),
        "expected description validation error, got {errors:?}"
    );
}

#[test]
fn validate_accepts_minimal_skill() {
    let mut temp = tempfile::NamedTempFile::new().expect("temp file");
    write!(
            temp,
            "---\nname: spec-test-skill\ndescription: Test specification skill behavior. Use when validating schema tests.\n---\n"
        )
        .expect("write temp frontmatter");
    validate_frontmatter(temp.path(), SchemaId::Skill)
        .unwrap_or_else(|error| panic!("valid frontmatter should validate: {error:?}"));
}
