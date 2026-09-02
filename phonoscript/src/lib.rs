//! PhonoScript language and the shared phonological analysis engine.
//!
//! This crate is independent of PhonoScript GUI. The desktop application consumes
//! this public API; the language and engine never depend on the GUI.

pub mod document;
pub mod engine;
pub mod exact;
pub mod export;
pub mod learning;
pub mod model;
pub mod otsoft;
pub mod phonological_engine;
pub mod phonology;
pub mod phonoscript_analysis;
pub mod phonoscript_frontend;
pub mod phonoscript_runtime;
pub mod ranking;
pub mod reference_cases;
pub mod reference_conformance;
