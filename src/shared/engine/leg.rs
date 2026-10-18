//! Shared leg state machine (one external contest).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::events::Fault;
use super::types::{LegId, ParticipantId, Side};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LegPhase {
    PendingCreate,
    AwaitingStart,
    InProgress,
    /// Terminal phases
    Decisive,
    Draw,
    Forfeit,
    MutualVoid,
    FaultVoid,
}

impl LegPhase {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Decisive | Self::Draw | Self::Forfeit | Self::MutualVoid | Self::FaultVoid
        )
    }
}

/// Normalized outcome the engine understands (sports map into this).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LegOutcome {
    Ongoing,
    Decisive { winner: Side },
    Draw,
    /// Unilateral abandon / no-show — loser is the faulting side.
    Forfeit { loser: Side },
    MutualVoid,
    /// Invalid contest; replay without settling escrow.
    FaultVoid,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LegState {
    pub id: LegId,
    pub phase: LegPhase,
    pub config_index: u32,
    pub config: Value,
    pub external_ref: Option<String>,
    pub accept_deadline: Option<DateTime<Utc>>,
    pub outcome: Option<LegOutcome>,
    pub winner: Option<ParticipantId>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl LegState {
    pub fn new(config_index: u32, config: Value, now: DateTime<Utc>) -> Self {
        Self {
            id: LegId::new(),
            phase: LegPhase::PendingCreate,
            config_index,
            config,
            external_ref: None,
            accept_deadline: None,
            outcome: None,
            winner: None,
            created_at: now,
            updated_at: now,
        }
    }

    /// Mark leg as awaiting start after executor created the external contest.
    pub fn mark_awaiting_start(
        &mut self,
        external_ref: String,
        accept_deadline: DateTime<Utc>,
        now: DateTime<Utc>,
    ) {
        self.phase = LegPhase::AwaitingStart;
        self.external_ref = Some(external_ref);
        self.accept_deadline = Some(accept_deadline);
        self.updated_at = now;
    }

    pub fn mark_in_progress(&mut self, now: DateTime<Utc>) {
        if self.phase == LegPhase::AwaitingStart || self.phase == LegPhase::PendingCreate {
            self.phase = LegPhase::InProgress;
            self.updated_at = now;
        }
    }

    /// Apply a classified observation. Returns true if phase became terminal.
    pub fn apply_outcome(
        &mut self,
        outcome: LegOutcome,
        side_a: &ParticipantId,
        side_b: &ParticipantId,
        now: DateTime<Utc>,
    ) -> bool {
        if self.phase.is_terminal() {
            return true;
        }
        match &outcome {
            LegOutcome::Ongoing => {
                if self.phase == LegPhase::AwaitingStart {
                    self.phase = LegPhase::InProgress;
                }
                self.updated_at = now;
                false
            }
            LegOutcome::Decisive { winner } => {
                self.phase = LegPhase::Decisive;
                self.winner = Some(match winner {
                    Side::A => side_a.clone(),
                    Side::B => side_b.clone(),
                });
                self.outcome = Some(outcome);
                self.updated_at = now;
                true
            }
            LegOutcome::Draw => {
                self.phase = LegPhase::Draw;
                self.outcome = Some(outcome);
                self.updated_at = now;
                true
            }
            LegOutcome::Forfeit { loser } => {
                self.phase = LegPhase::Forfeit;
                let winner_side = match loser {
                    Side::A => Side::B,
                    Side::B => Side::A,
                };
                self.winner = Some(match winner_side {
                    Side::A => side_a.clone(),
                    Side::B => side_b.clone(),
                });
                self.outcome = Some(outcome);
                self.updated_at = now;
                true
            }
            LegOutcome::MutualVoid => {
                self.phase = LegPhase::MutualVoid;
                self.outcome = Some(outcome);
                self.updated_at = now;
                true
            }
            LegOutcome::FaultVoid => {
                self.phase = LegPhase::FaultVoid;
                self.outcome = Some(outcome);
                self.updated_at = now;
                true
            }
        }
    }

    /// Resolve accept/start timeout using abandon policy inputs.
    pub fn apply_deadline_timeout(
        &mut self,
        fault: Fault,
        side_a: &ParticipantId,
        side_b: &ParticipantId,
        now: DateTime<Utc>,
    ) -> bool {
        let outcome = match fault {
            Fault::Both | Fault::Unknown => LegOutcome::MutualVoid,
            Fault::SideA => LegOutcome::Forfeit { loser: Side::A },
            Fault::SideB => LegOutcome::Forfeit { loser: Side::B },
        };
        self.apply_outcome(outcome, side_a, side_b, now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use serde_json::json;

    #[test]
    fn decisive_sets_winner() {
        let now = Utc::now();
        let mut leg = LegState::new(0, json!({}), now);
        leg.mark_awaiting_start("g1".into(), now, now);
        let a = ParticipantId::new("a");
        let b = ParticipantId::new("b");
        assert!(leg.apply_outcome(LegOutcome::Decisive { winner: Side::A }, &a, &b, now));
        assert_eq!(leg.phase, LegPhase::Decisive);
        assert_eq!(leg.winner.as_ref().unwrap().0, "a");
    }

    #[test]
    fn forfeit_awards_opponent() {
        let now = Utc::now();
        let mut leg = LegState::new(0, json!({}), now);
        let a = ParticipantId::new("a");
        let b = ParticipantId::new("b");
        assert!(leg.apply_outcome(LegOutcome::Forfeit { loser: Side::B }, &a, &b, now));
        assert_eq!(leg.winner.as_ref().unwrap().0, "a");
    }
}
