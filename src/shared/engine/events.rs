//! Outbox / domain events emitted by the engine for sport executors.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::types::{LegId, MatchId, ParticipantId, Side, SportId, TournamentId};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EngineEvent {
    /// Backend/sport adapter should create the external contest for this leg.
    SpawnLeg {
        sport: SportId,
        match_id: MatchId,
        leg_id: LegId,
        tournament_id: Option<TournamentId>,
        /// Opaque sport config (time control, fixture id, …).
        config: Value,
        accept_deadline: DateTime<Utc>,
    },
    /// A leg reached a terminal engine outcome.
    LegTerminal {
        sport: SportId,
        match_id: MatchId,
        leg_id: LegId,
        outcome: String,
        winner: Option<ParticipantId>,
    },
    /// Entire match is finished (series or single).
    MatchTerminal {
        sport: SportId,
        match_id: MatchId,
        tournament_id: Option<TournamentId>,
        winner: Option<ParticipantId>,
        loser: Option<ParticipantId>,
        /// `won` | `void`
        result: MatchTerminalResult,
    },
    /// Winner advanced in a knockout bracket.
    AdvanceBracket {
        sport: SportId,
        tournament_id: TournamentId,
        from_match_id: MatchId,
        to_match_id: MatchId,
        winner: ParticipantId,
    },
    /// Tournament finished; placements ready for settle attestation.
    TournamentComplete {
        sport: SportId,
        tournament_id: TournamentId,
        /// Ordered placements: index 0 = 1st place.
        placements: Vec<ParticipantId>,
    },
    Notify {
        sport: SportId,
        match_id: Option<MatchId>,
        tournament_id: Option<TournamentId>,
        participants: Vec<ParticipantId>,
        kind: NotifyKind,
        message: String,
    },
    ReadyToSettle {
        sport: SportId,
        match_id: Option<MatchId>,
        tournament_id: Option<TournamentId>,
        wager_id: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatchTerminalResult {
    Won,
    Void,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotifyKind {
    SpawnLeg,
    Forfeit,
    MatchWon,
    MatchVoid,
    BracketAdvance,
}

/// Who faulted when classifying abandon / no-show.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Fault {
    SideA,
    SideB,
    Both,
    Unknown,
}

impl Fault {
    pub fn as_side(self) -> Option<Side> {
        match self {
            Self::SideA => Some(Side::A),
            Self::SideB => Some(Side::B),
            Self::Both | Self::Unknown => None,
        }
    }
}
