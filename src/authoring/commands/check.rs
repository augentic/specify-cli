use specify_authoring::check;
use specify_authoring::context::Context;
use specify_authoring::error::ToolingError;
use specify_authoring::exit::{Exit, exit_from_result};
use specify_authoring::finding::Finding;

use crate::authoring::map_finding::map_findings;
use crate::authoring::output::{check_body, review_findings_body, write_check_text};
use crate::output::{self, Format};

pub fn run(format: Format, framework_root: std::path::PathBuf) -> Exit {
    let result = (|| -> Result<(std::path::PathBuf, Vec<Finding>), ToolingError> {
        let ctx = Context::from_framework_root(framework_root)?;
        let framework_root = ctx.framework_root().to_path_buf();
        Ok((framework_root, check::run(&ctx)))
    })();

    match format {
        Format::Text => emit_text(format, &result),
        Format::Json => emit_json(format, &result),
    }

    match result {
        Ok((_, findings)) => exit_from_result(Ok(()), findings.len()),
        Err(error) => exit_from_result(Err(error), 0),
    }
}

fn emit_text(format: Format, result: &Result<(std::path::PathBuf, Vec<Finding>), ToolingError>) {
    let body = check_body(result);
    if let Err(error) =
        output::emit(Box::new(std::io::stdout().lock()), format, &body, |w, body| {
            write_check_text(w, body, result)
        })
    {
        eprintln!("error: {error}");
    }
}

fn emit_json(format: Format, result: &Result<(std::path::PathBuf, Vec<Finding>), ToolingError>) {
    let findings = match result {
        Ok((_, findings)) => map_findings(findings.as_slice()),
        Err(_) => Vec::new(),
    };
    let body = review_findings_body(findings);
    if let Err(error) =
        output::emit(Box::new(std::io::stdout().lock()), format, &body, |_, _| Ok(()))
    {
        eprintln!("error: {error}");
    }
    if let Err(error) = result {
        eprintln!("error: {error}");
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use serde_json::Value as JsonValue;
    use specify_authoring::finding::{Finding, Location};
    use specify_lints::{Severity, validate_finding};

    use crate::authoring::map_finding::map_findings;
    use crate::authoring::output::review_findings_body;

    fn fixture(rule_id: &'static str, message: &str) -> Finding {
        Finding {
            rule_id,
            message: message.to_owned(),
            location: Some(Location {
                path: PathBuf::from("plugins/spec/skills/build/SKILL.md"),
                line: 1,
                column: None,
            }),
        }
    }

    /// (1) Empty inputs collapse to an envelope with all-zero summary
    /// counts and an empty `findings` array — the shape consumers see
    /// on a clean run.
    #[test]
    fn empty_findings_yields_zero_summary_and_empty_array() {
        let body = review_findings_body(Vec::new());
        assert_eq!(body.version, 1);
        assert_eq!(body.summary.critical, 0);
        assert_eq!(body.summary.important, 0);
        assert_eq!(body.summary.suggestion, 0);
        assert_eq!(body.summary.optional, 0);
        assert!(body.findings.is_empty());
    }

    /// (2) Mixed severities are counted correctly: one Critical
    /// (`rules.schema-violation`) and two Important
    /// (`skill.duplicate-name`) findings produce the expected per-key
    /// counts.
    #[test]
    fn mixed_severities_are_counted_correctly() {
        let inputs = vec![
            fixture("rules.schema-violation", "schema breakage"),
            fixture("skill.duplicate-name", "dup one"),
            fixture("skill.duplicate-name", "dup two"),
        ];
        let body = review_findings_body(map_findings(&inputs));

        assert_eq!(body.findings.len(), 3);
        assert_eq!(body.summary.critical, 1);
        assert_eq!(body.summary.important, 2);
        assert_eq!(body.summary.suggestion, 0);
        assert_eq!(body.summary.optional, 0);
    }

    /// (3) The serialized envelope exposes exactly `version`,
    /// `summary`, and `findings` at the top level, and the summary
    /// object exposes all four severity keys with `usize` counts.
    #[test]
    fn serialized_envelope_has_expected_top_level_keys() {
        let body = review_findings_body(map_findings(&[fixture("skill.duplicate-name", "dup")]));
        let value = serde_json::to_value(&body).expect("envelope serializes");
        let object = value.as_object().expect("envelope is an object");

        let mut keys: Vec<&str> = object.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, vec!["findings", "summary", "version"]);

        assert_eq!(object.get("version"), Some(&JsonValue::from(1_u32)));
        let summary =
            object.get("summary").and_then(JsonValue::as_object).expect("summary is an object");
        let mut summary_keys: Vec<&str> = summary.keys().map(String::as_str).collect();
        summary_keys.sort_unstable();
        assert_eq!(summary_keys, vec!["critical", "important", "optional", "suggestion"]);

        let findings =
            object.get("findings").and_then(JsonValue::as_array).expect("findings is an array");
        assert_eq!(findings.len(), 1);
    }

    /// (4) Every mapped finding inside the envelope passes CH-16's
    /// `validate_finding` schema check — the envelope ships only
    /// schema-valid findings.
    #[test]
    fn every_finding_in_envelope_validates_against_schema() {
        let inputs = vec![
            fixture("rules.schema-violation", "schema breakage"),
            fixture("skill.duplicate-name", "duplicate"),
            fixture("links.unresolved", "broken markdown link"),
        ];
        let body = review_findings_body(map_findings(&inputs));
        for finding in &body.findings {
            validate_finding(finding)
                .unwrap_or_else(|err| panic!("{} must validate: {err}", finding.id));
        }
    }

    /// (5) The serialized envelope matches the `LintResult`
    /// envelope" example shape: exactly the `version`, `summary`, and
    /// `findings` keys, with the documented per-severity summary
    /// fields and counts derived from the input.
    #[test]
    fn envelope_matches_contract_example_shape() {
        let inputs = vec![
            fixture("skill.duplicate-name", "dup one"),
            fixture("skill.duplicate-name", "dup two"),
        ];
        let body = review_findings_body(map_findings(&inputs));
        let value = serde_json::to_value(&body).expect("serialize");

        let summary = value.get("summary").expect("summary present");
        assert_eq!(summary.get("critical"), Some(&JsonValue::from(0_u32)));
        assert_eq!(summary.get("important"), Some(&JsonValue::from(2_u32)));
        assert_eq!(summary.get("suggestion"), Some(&JsonValue::from(0_u32)));
        assert_eq!(summary.get("optional"), Some(&JsonValue::from(0_u32)));
        assert_eq!(value.get("version"), Some(&JsonValue::from(1_u32)));
        assert!(value.get("findings").and_then(JsonValue::as_array).is_some());
    }

    /// (6) Severity wire-up sanity: a `rules.schema-violation` is
    /// counted under `critical`, exercising both the CH-20 severity
    /// table and the body's match arms.
    #[test]
    fn critical_authoring_rule_lands_in_critical_bucket() {
        let mapped = map_findings(&[fixture("rules.schema-violation", "boom")]);
        assert_eq!(mapped[0].severity, Severity::Critical);

        let body = review_findings_body(mapped);
        assert_eq!(body.summary.critical, 1);
        assert_eq!(body.summary.important, 0);
    }
}
