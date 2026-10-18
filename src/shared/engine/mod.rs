//! Game-agnostic tournament / match engine.
//!
//! Sports implement [`traits::LegRuntime`] + [`traits::OutcomeClassifier`] and
//! call into [`series::SeriesState`] / [`tournament::TournamentState`].

pub mod bracket;
pub mod events;
pub mod handler;
pub mod leg;
pub mod policy;
pub mod postgres;
pub mod router;
pub mod series;
pub mod service;
pub mod store;
pub mod tournament;
pub mod traits;
pub mod types;

pub use bracket::Bracket;
pub use events::{EngineEvent, Fault, MatchTerminalResult, NotifyKind};
pub use leg::{LegOutcome, LegPhase, LegState};
pub use policy::{
    AbandonPolicy, DeadlinePolicy, DrawPolicy, SeriesConfig, UnilateralAbandonScope,
    MAX_ACCEPT_DEADLINE_SECS, MIN_ACCEPT_DEADLINE_SECS,
};
pub use series::{SeriesPhase, SeriesState, SeriesTransition};
pub use store::{EngineStore, MemoryStore, OutboxRow, StoreError};
pub use tournament::{TournamentPhase, TournamentState};
pub use traits::{
    BracketRules, DefaultBracketRules, DefaultSeriesRules, LegObservation, LegRuntime,
    OutcomeClassifier, SeriesRules, SpawnCtx,
};
pub use types::*;
