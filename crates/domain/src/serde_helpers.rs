//! `skip_serializing_if` predicates shared across the domain crate.
//! `serde` requires `Fn(&T) -> bool`, so predicates take `&bool`
//! rather than by value.

/// `skip_serializing_if` predicate that omits a `bool` field when it is `false`.
#[must_use]
pub const fn is_false(value: &bool) -> bool {
    !*value
}
