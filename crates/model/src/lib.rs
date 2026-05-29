//! Specify model — artifact types and parsers (`spec`, `task`,
//! `evidence`, `discovery`) plus the shared atomic writer. A
//! lifecycle-free leaf beneath `specify-workflow` and
//! `specify-validate`: nothing here can transition a slice or stamp a
//! plan. See `docs/standards/architecture.md` for the rationale.

pub mod atomic;
pub mod discovery;
pub mod evidence;
pub mod spec;
pub mod task;
