//! Cage / multi-leg series scoring (sport-agnostic).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::events::{EngineEvent, MatchTerminalResult, NotifyKind};
use super::leg::{LegOutcome, LegPhase, LegState};
use super::policy::{DrawPolicy, SeriesConfig};
use super::types::{MatchId, MatchKind, ParticipantId, Side, SportId, TournamentId};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SeriesPhase {
    Open,
    Locked,
    LegActive,
    BetweenLegs,
    MatchWon,
    MatchVoid,
}

impl SeriesPhase {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::MatchWon | Self::MatchVoid)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SeriesTransition {
    Continue { spawn_next: bool, config_index: u32 },
    Won { winner: Side },
    Void,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SeriesState {
    pub match_id: MatchId,
    pub sport: SportId,
    pub tournament_id: Option<TournamentId>,
    /// On-chain wager id (0x…).
    pub wager_id: String,
    pub kind: MatchKind,
    pub phase: SeriesPhase,
    pub side_a: ParticipantId,
    pub side_b: ParticipantId,
    pub score_a: u32,
    pub score_b: u32,
    pub config: SeriesConfig,
    /// Per-leg sport configs (e.g. time controls). Index used when spawning.
    pub leg_configs: Vec<Value>,
    pub legs: Vec<LegState>,
    pub winner: Option<ParticipantId>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl SeriesState {
    pub fn new_series(
        sport: SportId,
        wager_id: String,
        side_a: ParticipantId,
        side_b: ParticipantId,
        config: SeriesConfig,
        leg_configs: Vec<Value>,
        tournament_id: Option<TournamentId>,
        now: DateTime<Utc>,
    ) -> Self {
        let leg_configs = if leg_configs.is_empty() {
            vec![Value::Object(Default::default())]
        } else {
            leg_configs
        };
        Self {
            match_id: MatchId::new(),
            sport,
            tournament_id,
            wager_id,
            kind: MatchKind::Series,
            phase: SeriesPhase::Open,
            side_a,
            side_b,
            score_a: 0,
            score_b: 0,
            config,
            leg_configs,
            legs: vec![],
            winner: None,
            created_at: now,
            updated_at: now,
        }
    }

    pub fn new_single(
        sport: SportId,
        wager_id: String,
        side_a: ParticipantId,
        side_b: ParticipantId,
        leg_config: Value,
        deadlines: super::policy::DeadlinePolicy,
        tournament_id: Option<TournamentId>,
        now: DateTime<Utc>,
    ) -> Self {
        let mut config = SeriesConfig::first_to(1);
        config.deadlines = deadlines;
        Self {
            match_id: MatchId::new(),
            sport,
            tournament_id,
            wager_id,
            kind: MatchKind::Single,
            phase: SeriesPhase::Open,
            side_a,
            side_b,
            score_a: 0,
            score_b: 0,
            config,
            leg_configs: vec![leg_config],
            legs: vec![],
            winner: None,
            created_at: now,
            updated_at: now,
        }
    }

    pub fn lock_and_start(&mut self, now: DateTime<Utc>) -> Vec<EngineEvent> {
        self.phase = SeriesPhase::Locked;
        self.updated_at = now;
        self.spawn_leg_at(0, now)
    }

    fn current_config_index(&self) -> u32 {
        self.legs
            .last()
            .map(|l| l.config_index)
            .unwrap_or(0)
    }

    fn config_at(&self, index: u32) -> Value {
        let i = index as usize % self.leg_configs.len().max(1);
        self.leg_configs.get(i).cloned().unwrap_or(Value::Null)
    }

    pub fn spawn_leg_at(&mut self, config_index: u32, now: DateTime<Utc>) -> Vec<EngineEvent> {
        let config = self.config_at(config_index);
        let mut leg = LegState::new(config_index, config.clone(), now);
        let deadline = now + self.config.accept_deadline_duration();
        leg.accept_deadline = Some(deadline);
        let leg_id = leg.id;
        self.legs.push(leg);
        self.phase = SeriesPhase::LegActive;
        self.updated_at = now;

        vec![
            EngineEvent::SpawnLeg {
                sport: self.sport,
                match_id: self.match_id,
                leg_id,
                tournament_id: self.tournament_id,
                config,
                accept_deadline: deadline,
            },
            EngineEvent::Notify {
                sport: self.sport,
                match_id: Some(self.match_id),
                tournament_id: self.tournament_id,
                participants: vec![self.side_a.clone(), self.side_b.clone()],
                kind: NotifyKind::SpawnLeg,
                message: "Next match leg is ready — join before the deadline".into(),
            },
        ]
    }

    /// Executor confirms the external contest exists.
    pub fn on_leg_created(
        &mut self,
        leg_id: super::types::LegId,
        external_ref: String,
        now: DateTime<Utc>,
    ) -> Result<(), String> {
        let deadline = now + self.config.accept_deadline_duration();
        let leg = self
            .legs
            .iter_mut()
            .find(|l| l.id == leg_id)
            .ok_or_else(|| "unknown leg".to_string())?;
        leg.mark_awaiting_start(external_ref, deadline, now);
        self.updated_at = now;
        Ok(())
    }

    pub fn on_leg_observation(
        &mut self,
        leg_id: super::types::LegId,
        outcome: LegOutcome,
        now: DateTime<Utc>,
    ) -> Result<Vec<EngineEvent>, String> {
        if self.phase.is_terminal() {
            return Ok(vec![]);
        }
        let side_a = self.side_a.clone();
        let side_b = self.side_b.clone();
        let sport = self.sport;
        let match_id = self.match_id;

        let leg = self
            .legs
            .iter_mut()
            .find(|l| l.id == leg_id)
            .ok_or_else(|| "unknown leg".to_string())?;

        if !leg.apply_outcome(outcome.clone(), &side_a, &side_b, now) {
            self.updated_at = now;
            return Ok(vec![]);
        }

        let winner = leg.winner.clone();
        let outcome_label = format!("{:?}", leg.phase);
        let mut events = vec![EngineEvent::LegTerminal {
            sport,
            match_id,
            leg_id,
            outcome: outcome_label,
            winner: winner.clone(),
        }];

        let transition = self.transition_after_leg(&outcome);
        events.extend(self.apply_transition(transition, now));
        Ok(events)
    }

    pub fn transition_after_leg(&self, outcome: &LegOutcome) -> SeriesTransition {
        match outcome {
            LegOutcome::Ongoing => SeriesTransition::Continue {
                spawn_next: false,
                config_index: self.current_config_index(),
            },
            LegOutcome::MutualVoid => SeriesTransition::Void,
            LegOutcome::FaultVoid => SeriesTransition::Continue {
                spawn_next: true,
                config_index: self.current_config_index(),
            },
            LegOutcome::Draw => match self.config.draw {
                DrawPolicy::ReplayLeg => SeriesTransition::Continue {
                    spawn_next: true,
                    config_index: self.current_config_index(),
                },
                DrawPolicy::SkipToNextConfig => SeriesTransition::Continue {
                    spawn_next: true,
                    config_index: self.current_config_index().saturating_add(1),
                },
            },
            LegOutcome::Forfeit { loser } => {
                let winner_side = match loser {
                    Side::A => Side::B,
                    Side::B => Side::A,
                };
                match self.config.abandon.unilateral_scope {
                    // Abandoning any leg ends the whole cage match.
                    super::policy::UnilateralAbandonScope::EntireMatch => SeriesTransition::Won {
                        winner: winner_side,
                    },
                    // Abandoning only forfeits this leg; series may continue.
                    super::policy::UnilateralAbandonScope::CurrentLeg => {
                        let (mut a, mut b) = (self.score_a, self.score_b);
                        match winner_side {
                            Side::A => a += 1,
                            Side::B => b += 1,
                        }
                        let need = self.config.wins_needed();
                        if a >= need || b >= need || self.kind == MatchKind::Single {
                            SeriesTransition::Won {
                                winner: winner_side,
                            }
                        } else {
                            SeriesTransition::Continue {
                                spawn_next: true,
                                config_index: self.legs.len() as u32
                                    % self.leg_configs.len().max(1) as u32,
                            }
                        }
                    }
                }
            }
            LegOutcome::Decisive { winner } => {
                let winner_side = *winner;
                let (mut a, mut b) = (self.score_a, self.score_b);
                match winner_side {
                    Side::A => a += 1,
                    Side::B => b += 1,
                }
                let need = self.config.wins_needed();
                if a >= need || b >= need || self.kind == MatchKind::Single {
                    SeriesTransition::Won {
                        winner: winner_side,
                    }
                } else {
                    SeriesTransition::Continue {
                        spawn_next: true,
                        config_index: self.legs.len() as u32 % self.leg_configs.len().max(1) as u32,
                    }
                }
            }
        }
    }

    pub(crate) fn apply_transition(
        &mut self,
        transition: SeriesTransition,
        now: DateTime<Utc>,
    ) -> Vec<EngineEvent> {
        match transition {
            SeriesTransition::Void => {
                self.phase = SeriesPhase::MatchVoid;
                self.updated_at = now;
                vec![
                    EngineEvent::MatchTerminal {
                        sport: self.sport,
                        match_id: self.match_id,
                        tournament_id: self.tournament_id,
                        winner: None,
                        loser: None,
                        result: MatchTerminalResult::Void,
                    },
                    EngineEvent::ReadyToSettle {
                        sport: self.sport,
                        match_id: Some(self.match_id),
                        tournament_id: self.tournament_id,
                        wager_id: self.wager_id.clone(),
                    },
                    EngineEvent::Notify {
                        sport: self.sport,
                        match_id: Some(self.match_id),
                        tournament_id: self.tournament_id,
                        participants: vec![self.side_a.clone(), self.side_b.clone()],
                        kind: NotifyKind::MatchVoid,
                        message: "Match voided — stakes can be refunded".into(),
                    },
                ]
            }
            SeriesTransition::Won { winner } => {
                match winner {
                    Side::A => self.score_a += 1,
                    Side::B => self.score_b += 1,
                }
                let (w, l) = match winner {
                    Side::A => (self.side_a.clone(), self.side_b.clone()),
                    Side::B => (self.side_b.clone(), self.side_a.clone()),
                };
                self.winner = Some(w.clone());
                self.phase = SeriesPhase::MatchWon;
                self.updated_at = now;
                vec![
                    EngineEvent::MatchTerminal {
                        sport: self.sport,
                        match_id: self.match_id,
                        tournament_id: self.tournament_id,
                        winner: Some(w.clone()),
                        loser: Some(l),
                        result: MatchTerminalResult::Won,
                    },
                    EngineEvent::ReadyToSettle {
                        sport: self.sport,
                        match_id: Some(self.match_id),
                        tournament_id: self.tournament_id,
                        wager_id: self.wager_id.clone(),
                    },
                    EngineEvent::Notify {
                        sport: self.sport,
                        match_id: Some(self.match_id),
                        tournament_id: self.tournament_id,
                        participants: vec![self.side_a.clone(), self.side_b.clone()],
                        kind: NotifyKind::MatchWon,
                        message: format!("{} won the match", w.0),
                    },
                ]
            }
            SeriesTransition::Continue {
                spawn_next,
                config_index,
            } => {
                // Update score for decisive/forfeit before continuing.
                if let Some(leg) = self.legs.last() {
                    if matches!(leg.phase, LegPhase::Decisive | LegPhase::Forfeit) {
                        if let Some(ref w) = leg.winner {
                            if w == &self.side_a {
                                self.score_a += 1;
                            } else if w == &self.side_b {
                                self.score_b += 1;
                            }
                        }
                    }
                }
                // Re-check win after score bump (forfeit mid-series).
                let need = self.config.wins_needed();
                if self.score_a >= need || self.score_b >= need {
                    let winner = if self.score_a >= need {
                        Side::A
                    } else {
                        Side::B
                    };
                    return self.apply_transition(SeriesTransition::Won { winner }, now);
                }
                if spawn_next {
                    self.phase = SeriesPhase::BetweenLegs;
                    self.updated_at = now;
                    self.spawn_leg_at(config_index, now)
                } else {
                    self.updated_at = now;
                    vec![]
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sides() -> (ParticipantId, ParticipantId) {
        (ParticipantId::new("alice"), ParticipantId::new("bob"))
    }

    #[test]
    fn first_to_two_spawns_until_win() {
        let now = Utc::now();
        let (a, b) = sides();
        let mut s = SeriesState::new_series(
            SportId::Lichess,
            "0x1".into(),
            a,
            b,
            SeriesConfig::first_to(2),
            vec![json!({"tc": "3+0"}), json!({"tc": "5+0"})],
            None,
            now,
        );
        let ev = s.lock_and_start(now);
        assert!(ev.iter().any(|e| matches!(e, EngineEvent::SpawnLeg { .. })));
        let leg1 = s.legs[0].id;
        let _ = s.on_leg_created(leg1, "g1".into(), now);
        let ev = s
            .on_leg_observation(leg1, LegOutcome::Decisive { winner: Side::A }, now)
            .unwrap();
        assert!(ev.iter().any(|e| matches!(e, EngineEvent::SpawnLeg { .. })));
        assert_eq!(s.score_a, 1);
        assert!(!s.phase.is_terminal());

        let leg2 = s.legs[1].id;
        let _ = s.on_leg_created(leg2, "g2".into(), now);
        let ev = s
            .on_leg_observation(leg2, LegOutcome::Decisive { winner: Side::A }, now)
            .unwrap();
        assert!(
            ev.iter()
                .any(|e| matches!(e, EngineEvent::MatchTerminal { .. }))
        );
        assert_eq!(s.phase, SeriesPhase::MatchWon);
        assert_eq!(s.score_a, 2);
    }

    #[test]
    fn unilateral_forfeit_ends_match() {
        let now = Utc::now();
        let (a, b) = sides();
        let mut s = SeriesState::new_series(
            SportId::Lichess,
            "0x1".into(),
            a,
            b,
            SeriesConfig::best_of(3),
            vec![json!({})],
            None,
            now,
        );
        s.lock_and_start(now);
        let leg1 = s.legs[0].id;
        let ev = s
            .on_leg_observation(leg1, LegOutcome::Forfeit { loser: Side::B }, now)
            .unwrap();
        assert_eq!(s.phase, SeriesPhase::MatchWon);
        assert_eq!(s.winner.as_ref().unwrap().0, "alice");
        assert!(
            ev.iter()
                .any(|e| matches!(e, EngineEvent::MatchTerminal { .. }))
        );
    }

    #[test]
    fn unilateral_forfeit_can_cost_only_leg() {
        use super::super::policy::UnilateralAbandonScope;
        let now = Utc::now();
        let (a, b) = sides();
        let mut config = SeriesConfig::best_of(3);
        config.abandon.unilateral_scope = UnilateralAbandonScope::CurrentLeg;
        let mut s = SeriesState::new_series(
            SportId::Lichess,
            "0x1".into(),
            a,
            b,
            config,
            vec![json!({})],
            None,
            now,
        );
        s.lock_and_start(now);
        let leg1 = s.legs[0].id;
        let ev = s
            .on_leg_observation(leg1, LegOutcome::Forfeit { loser: Side::B }, now)
            .unwrap();
        assert_eq!(s.score_a, 1);
        assert_eq!(s.score_b, 0);
        assert!(!s.phase.is_terminal());
        assert!(ev.iter().any(|e| matches!(e, EngineEvent::SpawnLeg { .. })));
    }

    #[test]
    fn mutual_void_voids_match() {
        let now = Utc::now();
        let (a, b) = sides();
        let mut s = SeriesState::new_series(
            SportId::Lichess,
            "0x1".into(),
            a,
            b,
            SeriesConfig::first_to(2),
            vec![json!({})],
            None,
            now,
        );
        s.lock_and_start(now);
        let leg1 = s.legs[0].id;
        let ev = s
            .on_leg_observation(leg1, LegOutcome::MutualVoid, now)
            .unwrap();
        assert_eq!(s.phase, SeriesPhase::MatchVoid);
        assert!(
            ev.iter()
                .any(|e| matches!(e, EngineEvent::MatchTerminal { result: MatchTerminalResult::Void, .. }))
        );
    }

    #[test]
    fn draw_replays_same_config() {
        let now = Utc::now();
        let (a, b) = sides();
        let mut s = SeriesState::new_series(
            SportId::Lichess,
            "0x1".into(),
            a,
            b,
            SeriesConfig::first_to(1),
            vec![json!({"tc": "10+0"})],
            None,
            now,
        );
        s.lock_and_start(now);
        let leg1 = s.legs[0].id;
        let ev = s.on_leg_observation(leg1, LegOutcome::Draw, now).unwrap();
        assert!(ev.iter().any(|e| matches!(e, EngineEvent::SpawnLeg { .. })));
        assert_eq!(s.legs.len(), 2);
        assert_eq!(s.legs[1].config_index, 0);
        assert!(!s.phase.is_terminal());
    }
}
