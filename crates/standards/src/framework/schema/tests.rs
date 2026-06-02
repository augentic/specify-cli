use std::io::Write;

use super::*;

#[test]
fn validate_rejects_invalid_description() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(tempdir.path().join("plugins")).expect("plugins");
    std::fs::create_dir_all(tempdir.path().join("adapters")).expect("adapters");
    let ctx = Context::from_framework_root(tempdir.path()).expect("framework root resolves");
    let mut temp = tempfile::NamedTempFile::new().expect("temp file");
    write!(temp, "---\nname: spec-test-skill\ndescription: Too short.\n---\n")
        .expect("write temp frontmatter");

    let result = validate_frontmatter(&ctx, temp.path(), SchemaId::Skill);
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
    let tempdir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(tempdir.path().join("plugins")).expect("plugins");
    std::fs::create_dir_all(tempdir.path().join("adapters")).expect("adapters");
    let ctx = Context::from_framework_root(tempdir.path()).expect("framework root resolves");
    let mut temp = tempfile::NamedTempFile::new().expect("temp file");
    write!(
            temp,
            "---\nname: spec-test-skill\ndescription: Test specification skill behavior. Use when validating schema tests.\n---\n"
        )
        .expect("write temp frontmatter");
    validate_frontmatter(&ctx, temp.path(), SchemaId::Skill)
        .unwrap_or_else(|error| panic!("valid frontmatter should validate: {error:?}"));
}
