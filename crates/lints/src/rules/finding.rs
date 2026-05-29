//! Diagnostic validation re-exports.
//!
//! The structured-diagnostic validators live in the neutral
//! [`specify_diagnostics`] leaf. This module re-exports them at the
//! `crate::rules::finding` path under their neutral names so the rules
//! layer and its consumers reach the shared
//! [`specify_diagnostics::validate()`] surface through one import path.

pub use specify_diagnostics::{
    DiagnosticError, validate, validate_diagnostic, validate_diagnostic_json,
    validate_evidence_size, validate_fingerprint,
};
