use super::*;

#[test]
fn transition_refining_to_refined_ok() {
    assert_eq!(
        LifecycleStatus::Refining.transition(LifecycleStatus::Refined).expect("legal"),
        LifecycleStatus::Refined,
    );
}

#[test]
fn transition_rejects_skipping_states() {
    let Err(Error::Diag { code, detail }) =
        LifecycleStatus::Refining.transition(LifecycleStatus::Built)
    else {
        panic!("Refining -> Built skips Refined; must Err with lifecycle diag");
    };
    assert_eq!(code, "lifecycle");
    assert!(detail.contains("Refining") && detail.contains("Built"), "endpoints in: {detail:?}");
}
