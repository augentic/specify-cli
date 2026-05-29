//! Diagnostic validation re-exports.
//!
//! The structured-finding validators now live in the neutral
//! [`specify_diagnostics`] leaf. This module re-exports them at the
//! historical `crate::rules::finding` path under the legacy
//! `*_finding` names so existing call sites keep resolving while the
//! codebase migrates to the neutral [`specify_diagnostics::validate()`]
//! surface.

pub use specify_diagnostics::{
    DiagnosticError as FindingError, validate, validate_diagnostic as validate_finding,
    validate_diagnostic_json as validate_finding_json, validate_evidence_size,
    validate_fingerprint,
};
