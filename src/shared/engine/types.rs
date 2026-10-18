//! Sport-agnostic identifiers and enums for the tournament/match engine.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Discriminator for which sport adapter owns a match/tournament.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SportId {
    Lichess,
    Football,
    Strava,
    Fpl,
}

impl SportId {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Lichess => "lichess",
            Self::Football => "football",
            Self::Strava => "strava",
            Self::Fpl => "fpl",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "lichess" => Some(Self::Lichess),
            "football" => Some(Self::Football),
            "strava" => Some(Self::Strava),
            "fpl" => Some(Self::Fpl),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ParticipantId(pub String);

impl ParticipantId {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MatchId(pub Uuid);

impl MatchId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for MatchId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TournamentId(pub Uuid);

impl TournamentId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for TournamentId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct LegId(pub Uuid);

impl LegId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for LegId {
    fn default() -> Self {
        Self::new()
    }
}

/// How a match is structured under a tournament (or standalone).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatchKind {
    /// One external contest decides the match.
    Single,
    /// Multi-leg series (cage).
    Series,
}

/// Series win condition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SeriesRule {
    FirstTo,
    BestOf,
}

/// High-level tournament format plugins.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TournamentFormat {
    Knockout,
    /// Reserved for future adapters (arena already exists elsewhere).
    Arena,
}

/// Side index within a 1v1 match (adapter maps to sport colors/roles).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    A,
    B,
}
