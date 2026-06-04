//! Parity tests: each declarative `CORE-*` rule must flag the same cases
//! as its retiring (or notional) imperative predicate.
//!
//! Each `core_*` submodule (one file under `core_parity/`) stages a
//! synthetic fixture, runs an inline reference implementation of the
//! imperative predicate semantics, runs the declarative pipeline
//! (`lint::index::build` + `lint::eval::evaluate`) against a synthesised
//! rule carrying the hints the `CORE-*` rule ships on disk, and asserts
//! both passes agree on the flagged set. Per-finding locations are not
//! compared byte-identically — functional parity (which cases were
//! flagged) is the contract.
//!
//! The shared `make_rule` / `hint` / `hint_with_config` / `NoToolRunner`
//! scaffolding is single-sourced in `eval_support`.

mod eval_support;

#[path = "core_parity/core_001.rs"]
mod core_001;
#[path = "core_parity/core_002.rs"]
mod core_002;
#[path = "core_parity/core_003.rs"]
mod core_003;
#[path = "core_parity/core_004.rs"]
mod core_004;
#[path = "core_parity/core_005.rs"]
mod core_005;
#[path = "core_parity/core_006.rs"]
mod core_006;
#[path = "core_parity/core_007.rs"]
mod core_007;
#[path = "core_parity/core_008.rs"]
mod core_008;
#[path = "core_parity/core_009.rs"]
mod core_009;
#[path = "core_parity/core_014.rs"]
mod core_014;
#[path = "core_parity/core_016.rs"]
mod core_016;
#[path = "core_parity/core_023.rs"]
mod core_023;
#[path = "core_parity/core_025.rs"]
mod core_025;
#[path = "core_parity/core_037.rs"]
mod core_037;
#[path = "core_parity/core_038.rs"]
mod core_038;
#[path = "core_parity/core_050.rs"]
mod core_050;
