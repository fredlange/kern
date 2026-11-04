//! The Kern toolchain (Rust port of kern/*.py; docs/research/RUST_PORT_PLAN.md).
//!
//! Crate-wide rule: no std HashMap/HashSet where iteration order can reach Z3 terms, mutants or output text
//! (FZ finding 6). IndexMap keeps Python's dict order; BTreeMap where Python sorts.

/// Kern's Int. Python's interpreter has unbounded ints and kern-rt runs i64; which one the language means is open
/// decision O1 in the plan. Until it is decided every Int goes through this alias, and arithmetic is checked.
pub type Int = i128;

pub mod pyrepr;
pub mod syntax;
pub mod pyrandom;
pub mod run;
pub mod infer;
pub mod wf;
pub mod why;
pub mod report;
pub mod package;
pub mod cli;
pub mod zz;
pub mod views;
pub mod render;
pub mod baseline;
pub mod prove;
pub mod oracle;
pub mod emit_rust;
pub mod pyjson;
pub mod host;
