//! Lichess adapter for the shared tournament/match engine.

pub mod classifier;
pub mod runtime;
pub mod watcher;

pub use classifier::LichessOutcomeClassifier;
pub use runtime::LichessLegRuntime;
