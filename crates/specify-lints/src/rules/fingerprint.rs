//! Diagnostic fingerprint re-exports.
//!
//! The fingerprint algorithm and canonical-JSON helpers now live in
//! the neutral [`specify_diagnostics`] leaf so the `validate` surface
//! can fingerprint diagnostics without depending on anything named
//! `lint`. This module re-exports them at the historical
//! `crate::rules::fingerprint` path so existing call sites keep
//! resolving.

pub use specify_diagnostics::{canonical_json, fingerprint, verify_fingerprint};
