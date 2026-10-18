//! Traits sport adapters implement to plug into the shared engine.

use chrono::{DateTime, Utc};
use serde_json::Value;

use super::bracket::Bracket;
use super::leg::LegOutcome;
use super::policy::SeriesConfig;
use super::series::{SeriesState, SeriesTransition};
use super::types::{MatchId, ParticipantId, SportId};

/// Context passed when spawning a leg.
#[derive(Debug, Clone)]
pub struct SpawnCtx {
    pub sport: SportId,
    pub match_id: MatchId,
    pub side_a: ParticipantId,
    pub side_b: ParticipantId,
    pub wager_id: String,
}

/// Observation returned by polling an external contest.
#[derive(Debug, Clone)]
pub struct LegObservation {
    pub raw: Value,
    pub outcome: LegOutcome,
    pub observed_at: DateTime<Utc>,
}

/// How this sport creates and watches a single competitive unit ("leg").
#[allow(dead_code)]
pub trait LegRuntime: Send + Sync {
    /// Opaque sport config (JSON).
    fn sport_id(&self) -> SportId;

    /// Create the external contest. Returns an external reference string
    /// (Lichess challenge/game id, fixture id, …).
    ///
    /// MVP: adapters may return a "pending" ref and let the backend executor
    /// finish creation, then call `SeriesState::on_leg_created`.
    fn describe_spawn(&self, ctx: &SpawnCtx, config: &Value) -> Value {
        let _ = (ctx, config);
        Value::Null
    }
}

/// Map sport-native signals → engine `LegOutcome`.
pub trait OutcomeClassifier: Send + Sync {
    fn classify_value(&self, raw: &Value) -> LegOutcome;
}

/// Optional sport-specific series scoring. Default uses shared first_to / best_of.
#[allow(dead_code)]
pub trait SeriesRules: Send + Sync {
    fn on_leg_terminal(
        &self,
        series: &SeriesState,
        outcome: &LegOutcome,
    ) -> SeriesTransition {
        series.transition_after_leg(outcome)
    }

    fn config(&self) -> &SeriesConfig;
}

/// Default series rules wrapping a `SeriesConfig`.
pub struct DefaultSeriesRules {
    pub config: SeriesConfig,
}

impl SeriesRules for DefaultSeriesRules {
    fn config(&self) -> &SeriesConfig {
        &self.config
    }
}

/// Bracket / tournament customization.
#[allow(dead_code)]
pub trait BracketRules: Send + Sync {
    fn seed(&self, entrants: &[ParticipantId], match_kind: super::types::MatchKind) -> Bracket {
        Bracket::seed_single_elim(entrants, match_kind)
    }

    fn on_match_won(
        &self,
        bracket: &mut Bracket,
        match_id: MatchId,
        winner: ParticipantId,
    ) -> Result<Option<MatchId>, String> {
        bracket.advance(match_id, winner)
    }
}

pub struct DefaultBracketRules;

impl BracketRules for DefaultBracketRules {}
