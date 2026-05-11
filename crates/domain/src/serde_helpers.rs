//! `skip_serializing_if` predicates shared across the domain crate.
//!
//! `serde`'s `skip_serializing_if` requires `Fn(&T) -> bool`, so the
//! `&bool` parameter is forced — we can't take by value.

/// `skip_serializing_if` predicate that omits a `bool` field when it is `false`.
#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde's skip_serializing_if requires `Fn(&T) -> bool`."
)]
#[must_use]
pub const fn is_false(value: &bool) -> bool {
    !*value
}
