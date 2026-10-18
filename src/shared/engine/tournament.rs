//! Tournament lifecycle wrapping a bracket (and future formats).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::bracket::Bracket;
use super::events::EngineEvent;
use super::types::{
    MatchId, MatchKind, ParticipantId, SportId, TournamentFormat, TournamentId,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TournamentPhase {
    Draft,
    Registration,
    Seeding,
    RoundLive,
    Complete,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TournamentState {
    pub id: TournamentId,
    pub sport: SportId,
    pub format: TournamentFormat,
    pub phase: TournamentPhase,
    /// On-chain multi-party wager id.
    pub wager_id: String,
    pub entrants: Vec<ParticipantId>,
    /// Default kind when `round_kinds` is empty.
    pub match_kind: MatchKind,
    /// Optional per-round kinds (round 0 = first round). Empty → all use `match_kind`.
    #[serde(default)]
    pub round_kinds: Vec<MatchKind>,
    pub series_n: Option<u32>,
    pub series_rule: Option<super::types::SeriesRule>,
    pub leg_configs: Vec<Value>,
    pub bracket: Option<Bracket>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl TournamentState {
    pub fn new_knockout(
        sport: SportId,
        wager_id: String,
        match_kind: MatchKind,
        series_n: Option<u32>,
        series_rule: Option<super::types::SeriesRule>,
        leg_configs: Vec<Value>,
        now: DateTime<Utc>,
    ) -> Self {
        Self::new_knockout_with_rounds(
            sport,
            wager_id,
            match_kind,
            vec![],
            series_n,
            series_rule,
            leg_configs,
            now,
        )
    }

    pub fn new_knockout_with_rounds(
        sport: SportId,
        wager_id: String,
        match_kind: MatchKind,
        round_kinds: Vec<MatchKind>,
        series_n: Option<u32>,
        series_rule: Option<super::types::SeriesRule>,
        leg_configs: Vec<Value>,
        now: DateTime<Utc>,
    ) -> Self {
        Self {
            id: TournamentId::new(),
            sport,
            format: TournamentFormat::Knockout,
            phase: TournamentPhase::Draft,
            wager_id,
            entrants: vec![],
            match_kind,
            round_kinds,
            series_n,
            series_rule,
            leg_configs,
            bracket: None,
            created_at: now,
            updated_at: now,
        }
    }

    pub fn open_registration(&mut self, now: DateTime<Utc>) {
        self.phase = TournamentPhase::Registration;
        self.updated_at = now;
    }

    pub fn register(&mut self, p: ParticipantId, now: DateTime<Utc>) -> Result<(), String> {
        if self.phase != TournamentPhase::Registration {
            return Err("registration closed".into());
        }
        if self.entrants.iter().any(|e| e == &p) {
            return Err("already registered".into());
        }
        self.entrants.push(p);
        self.updated_at = now;
        Ok(())
    }

    pub fn seed(&mut self, now: DateTime<Utc>) -> Result<Vec<MatchId>, String> {
        if self.entrants.len() < 2 {
            return Err("need at least 2 entrants".into());
        }
        self.phase = TournamentPhase::Seeding;
        let kinds = if self.round_kinds.is_empty() {
            vec![self.match_kind]
        } else {
            self.round_kinds.clone()
        };
        let bracket = Bracket::seed_single_elim_rounds(&self.entrants, &kinds);
        let ready: Vec<MatchId> = bracket.ready_matches().iter().map(|s| s.match_id).collect();
        self.bracket = Some(bracket);
        self.phase = TournamentPhase::RoundLive;
        self.updated_at = now;
        Ok(ready)
    }

    pub fn on_match_won(
        &mut self,
        from_match_id: MatchId,
        winner: ParticipantId,
        now: DateTime<Utc>,
    ) -> Result<(Vec<EngineEvent>, Option<MatchId>), String> {
        let sport = self.sport;
        let tid = self.id;
        let bracket = self
            .bracket
            .as_mut()
            .ok_or_else(|| "no bracket".to_string())?;
        let next = bracket.advance(from_match_id, winner.clone())?;
        let mut events = vec![EngineEvent::AdvanceBracket {
            sport,
            tournament_id: tid,
            from_match_id,
            to_match_id: next.unwrap_or(from_match_id),
            winner: winner.clone(),
        }];

        if bracket.final_winner().is_some() {
            self.phase = TournamentPhase::Complete;
            let placements = bracket.placements();
            events.push(EngineEvent::TournamentComplete {
                sport,
                tournament_id: tid,
                placements: placements.clone(),
            });
            events.push(EngineEvent::ReadyToSettle {
                sport,
                match_id: None,
                tournament_id: Some(tid),
                wager_id: self.wager_id.clone(),
            });
        }
        self.updated_at = now;
        Ok((events, next))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[test]
    fn knockout_seeds_and_completes() {
        let now = Utc::now();
        let mut t = TournamentState::new_knockout(
            SportId::Lichess,
            "0xabc".into(),
            MatchKind::Single,
            None,
            None,
            vec![],
            now,
        );
        t.open_registration(now);
        t.register(ParticipantId::new("a"), now).unwrap();
        t.register(ParticipantId::new("b"), now).unwrap();
        t.register(ParticipantId::new("c"), now).unwrap();
        t.register(ParticipantId::new("d"), now).unwrap();
        let ready = t.seed(now).unwrap();
        assert_eq!(ready.len(), 2);
        let m0 = ready[0];
        let m1 = ready[1];
        let (ev, _) = t
            .on_match_won(m0, ParticipantId::new("a"), now)
            .unwrap();
        assert!(ev.iter().any(|e| matches!(e, EngineEvent::AdvanceBracket { .. })));
        let _ = t.on_match_won(m1, ParticipantId::new("c"), now).unwrap();
        let final_ready = t.bracket.as_ref().unwrap().ready_matches();
        assert_eq!(final_ready.len(), 1);
        let fid = final_ready[0].match_id;
        let (ev, _) = t
            .on_match_won(fid, ParticipantId::new("a"), now)
            .unwrap();
        assert_eq!(t.phase, TournamentPhase::Complete);
        assert!(
            ev.iter()
                .any(|e| matches!(e, EngineEvent::TournamentComplete { .. }))
        );
    }
}
