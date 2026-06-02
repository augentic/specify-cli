use super::*;

#[test]
fn arg_hint_accepts_valid() {
    assert!(argument_hint_grammar_error("<slice-dir>").is_none());
    assert!(argument_hint_grammar_error("[crate-name]").is_none());
    assert!(argument_hint_grammar_error("<a|b|c>").is_none());
    assert!(argument_hint_grammar_error("--kind <kind>").is_none());
}

#[test]
fn arg_hint_rejects_prose() {
    assert_eq!(argument_hint_grammar_error("the slice name"), Some("the".to_string()));
}
