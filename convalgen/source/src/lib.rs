//! PhonoScript GUI: native desktop analysis built on the independent PhonoScript engine.

pub mod app;
#[cfg(target_os = "macos")]
pub mod macos;
pub mod phonoscript_editor;
pub mod theme;

pub use phonoscript::{
    document, engine, exact, export, learning, model, otsoft, phonological_engine, phonology,
    phonoscript_analysis, phonoscript_frontend, phonoscript_runtime, ranking, reference_cases,
    reference_conformance,
};
