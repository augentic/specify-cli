//! YAML wrapper that hides `serde_saphyr` from public surfaces, paired
//! with [`crate::Error`]'s `Yaml` variant. Covers both deserialization
//! and serialization paths through a single enum.

/// Wrapper around [`serde_saphyr`] (de)serialization errors.
#[derive(Debug, thiserror::Error)]
pub enum YamlError {
    /// Deserialization error from `serde_saphyr::from_str`.
    #[error(transparent)]
    De(#[from] serde_saphyr::Error),
    /// Serialization error from `serde_saphyr::to_string`.
    #[error(transparent)]
    Ser(#[from] serde_saphyr::ser::Error),
}
