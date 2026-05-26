//! Byte-preserving parser and write planner for the fenced `AGENTS.md`
//! context block. [`parse`] decodes a candidate document; [`render`]
//! composes the final write plan that preserves operator-authored bytes.

mod parse;
mod render;

pub(super) use parse::{FenceError, parse_document};
pub(super) use render::{WriteDisposition, plan_agents_write};
