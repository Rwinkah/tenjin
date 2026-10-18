//! High-level engine operations used by HTTP handlers.

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;

use super::events::{EngineEvent, Fault};
use super::leg::LegOutcome;
use super::policy::{DeadlinePolicy, SeriesConfig};
use super::series::{SeriesPhase, SeriesState};
use super::store::{EngineStore, OutboxRow, StoreError};
use super::tournament::TournamentState;
use super::types::{
    MatchId, MatchKind, ParticipantId, SeriesRule, SportId, TournamentId,
};
use super::traits::OutcomeClassifier;

#[derive(Clone)]
pub struct EngineService {
    store: Arc<dyn EngineStore>,
}

impl EngineService {
    pub fn new(store: Arc<dyn EngineStore>) -> Self {
        Self { store }
    }

    pub fn store(&self) -> &Arc<dyn EngineStore> {
        &self.store
    }

    pub async fn create_match(&self, req: CreateMatchRequest) -> Result<SeriesState, StoreError> {
        let now = Utc::now();
        let mut state = match req.kind {
            MatchKind::Single => SeriesState::new_single(
                req.sport,
                req.wager_id,
                ParticipantId::new(req.side_a),
                ParticipantId::new(req.side_b),
                req.leg_configs
                    .first()
                    .cloned()
                    .unwrap_or(Value::Object(Default::default())),
                req.deadlines.unwrap_or_default().clamped(),
                req.tournament_id,
                now,
            ),
            MatchKind::Series => {
                let config = match req.series_rule.unwrap_or(SeriesRule::FirstTo) {
                    SeriesRule::FirstTo => SeriesConfig::first_to(req.series_n.unwrap_or(2)),
                    SeriesRule::BestOf => SeriesConfig::best_of(req.series_n.unwrap_or(3)),
                };
                let mut config = config;
                if let Some(d) = req.deadlines {
                    config.deadlines = d.clamped();
                }
                if let Some(a) = req.abandon {
                    config.abandon = a;
                }
                let config = config.with_clamped_deadlines();
                SeriesState::new_series(
                    req.sport,
                    req.wager_id,
                    ParticipantId::new(req.side_a),
                    ParticipantId::new(req.side_b),
                    config,
                    req.leg_configs,
                    req.tournament_id,
                    now,
                )
            }
        };

        // Optionally replace generated match id with client-provided.
        if let Some(id) = req.match_id {
            state.match_id = id;
        }

        let events = if req.auto_start {
            state.lock_and_start(now)
        } else {
            vec![]
        };
        self.store.put_match(&state).await?;
        if !events.is_empty() {
            self.store.push_events(&events).await?;
        }
        Ok(state)
    }

    pub async fn start_match(
        &self,
        sport: SportId,
        match_id: MatchId,
    ) -> Result<(SeriesState, Vec<EngineEvent>), StoreError> {
        let mut state = self.store.get_match(sport, match_id).await?;
        let events = state.lock_and_start(Utc::now());
        self.store.put_match(&state).await?;
        self.store.push_events(&events).await?;
        Ok((state, events))
    }

    pub async fn leg_created(
        &self,
        sport: SportId,
        match_id: MatchId,
        leg_id: super::types::LegId,
        external_ref: String,
    ) -> Result<SeriesState, StoreError> {
        let mut state = self.store.get_match(sport, match_id).await?;
        state
            .on_leg_created(leg_id, external_ref, Utc::now())
            .map_err(StoreError::Message)?;
        self.store.put_match(&state).await?;
        Ok(state)
    }

    pub async fn observe_leg(
        &self,
        sport: SportId,
        match_id: MatchId,
        leg_id: super::types::LegId,
        outcome: LegOutcome,
    ) -> Result<(SeriesState, Vec<EngineEvent>), StoreError> {
        let mut state = self.store.get_match(sport, match_id).await?;
        let mut events = state
            .on_leg_observation(leg_id, outcome, Utc::now())
            .map_err(StoreError::Message)?;
        self.store.put_match(&state).await?;
        if !events.is_empty() {
            self.store.push_events(&events).await?;
        }

        // Knockout: a finished bracket match advances the bracket and may open the next slot.
        if state.phase == SeriesPhase::MatchWon {
            if let (Some(tid), Some(winner)) = (state.tournament_id, state.winner.clone()) {
                match self
                    .tournament_match_won(sport, tid, match_id, winner.0)
                    .await
                {
                    Ok((_, adv, _)) => events.extend(adv),
                    Err(StoreError::Message(msg)) if msg.contains("already decided") => {}
                    Err(err) => return Err(err),
                }
            }
        }

        Ok((state, events))
    }

    pub async fn observe_leg_raw(
        &self,
        sport: SportId,
        match_id: MatchId,
        leg_id: super::types::LegId,
        raw: &Value,
        classifier: &dyn OutcomeClassifier,
    ) -> Result<(SeriesState, Vec<EngineEvent>), StoreError> {
        let outcome = classifier.classify_value(raw);
        self.observe_leg(sport, match_id, leg_id, outcome).await
    }

    pub async fn deadline_timeout(
        &self,
        sport: SportId,
        match_id: MatchId,
        leg_id: super::types::LegId,
        fault: Fault,
    ) -> Result<(SeriesState, Vec<EngineEvent>), StoreError> {
        let outcome = match fault {
            Fault::Both | Fault::Unknown => LegOutcome::MutualVoid,
            Fault::SideA => LegOutcome::Forfeit { loser: super::types::Side::A },
            Fault::SideB => LegOutcome::Forfeit { loser: super::types::Side::B },
        };
        self.observe_leg(sport, match_id, leg_id, outcome).await
    }

    pub async fn create_tournament(
        &self,
        req: CreateTournamentRequest,
    ) -> Result<TournamentState, StoreError> {
        let now = Utc::now();
        let mut state = TournamentState::new_knockout_with_rounds(
            req.sport,
            req.wager_id,
            req.match_kind,
            req.round_kinds.unwrap_or_default(),
            req.series_n,
            req.series_rule,
            req.leg_configs,
            now,
        );
        if let Some(id) = req.tournament_id {
            state.id = id;
        }
        state.open_registration(now);
        for e in req.entrants {
            state
                .register(ParticipantId::new(e), now)
                .map_err(StoreError::Message)?;
        }
        self.store.put_tournament(&state).await?;
        Ok(state)
    }

    pub async fn register_entrant(
        &self,
        sport: SportId,
        tournament_id: TournamentId,
        entrant: String,
    ) -> Result<TournamentState, StoreError> {
        let mut state = self.store.get_tournament(sport, tournament_id).await?;
        state
            .register(ParticipantId::new(entrant), Utc::now())
            .map_err(StoreError::Message)?;
        self.store.put_tournament(&state).await?;
        Ok(state)
    }

    pub async fn seed_tournament(
        &self,
        sport: SportId,
        tournament_id: TournamentId,
    ) -> Result<(TournamentState, Vec<MatchId>), StoreError> {
        let mut state = self.store.get_tournament(sport, tournament_id).await?;
        let ready = state.seed(Utc::now()).map_err(StoreError::Message)?;
        self.store.put_tournament(&state).await?;
        // Create playable series/single matches for every ready bracket slot and emit SpawnLeg.
        self.spawn_ready_bracket_matches(&state, &ready).await?;
        let state = self.store.get_tournament(sport, tournament_id).await?;
        Ok((state, ready))
    }

    /// Materialize SeriesState + SpawnLeg for ready bracket slots that are missing matches.
    /// Safe to call after a prior seed that only partially created matches.
    pub async fn ensure_ready_matches(
        &self,
        sport: SportId,
        tournament_id: TournamentId,
    ) -> Result<(TournamentState, Vec<MatchId>), StoreError> {
        let state = self.store.get_tournament(sport, tournament_id).await?;
        let ready: Vec<MatchId> = state
            .bracket
            .as_ref()
            .map(|b| b.ready_matches().iter().map(|s| s.match_id).collect())
            .unwrap_or_default();
        self.spawn_ready_bracket_matches(&state, &ready).await?;
        let state = self.store.get_tournament(sport, tournament_id).await?;
        Ok((state, ready))
    }

    pub async fn tournament_match_won(
        &self,
        sport: SportId,
        tournament_id: TournamentId,
        from_match_id: MatchId,
        winner: String,
    ) -> Result<(TournamentState, Vec<EngineEvent>, Option<MatchId>), StoreError> {
        let mut state = self.store.get_tournament(sport, tournament_id).await?;
        let (events, next) = state
            .on_match_won(from_match_id, ParticipantId::new(winner), Utc::now())
            .map_err(StoreError::Message)?;
        self.store.put_tournament(&state).await?;
        self.store.push_events(&events).await?;
        if let Some(next_id) = next {
            self.spawn_ready_bracket_matches(&state, &[next_id]).await?;
        }
        let state = self.store.get_tournament(sport, tournament_id).await?;
        Ok((state, events, next))
    }

    /// Materialize engine matches for ready bracket slots (idempotent).
    async fn spawn_ready_bracket_matches(
        &self,
        tournament: &TournamentState,
        ready_ids: &[MatchId],
    ) -> Result<(), StoreError> {
        let bracket = tournament
            .bracket
            .as_ref()
            .ok_or_else(|| StoreError::Message("no bracket".into()))?;
        for mid in ready_ids {
            if self.store.get_match(tournament.sport, *mid).await.is_ok() {
                continue;
            }
            let slot = bracket
                .slots
                .iter()
                .find(|s| s.match_id == *mid)
                .ok_or_else(|| StoreError::Message("ready match missing from bracket".into()))?;
            let side_a = slot
                .side_a
                .as_ref()
                .ok_or_else(|| StoreError::Message("ready slot missing side_a".into()))?
                .0
                .clone();
            let side_b = slot
                .side_b
                .as_ref()
                .ok_or_else(|| StoreError::Message("ready slot missing side_b".into()))?
                .0
                .clone();
            let req = CreateMatchRequest {
                sport: tournament.sport,
                wager_id: tournament.wager_id.clone(),
                side_a,
                side_b,
                kind: slot.kind,
                series_rule: tournament.series_rule,
                series_n: tournament.series_n,
                leg_configs: tournament.leg_configs.clone(),
                deadlines: None,
                abandon: None,
                tournament_id: Some(tournament.id),
                match_id: Some(*mid),
                auto_start: true,
            };
            self.create_match(req).await?;
        }
        Ok(())
    }

    pub async fn pending_events(&self, limit: usize) -> Result<Vec<OutboxRow>, StoreError> {
        self.store.list_pending_events(limit).await
    }

    pub async fn ack_event(&self, id: uuid::Uuid) -> Result<(), StoreError> {
        self.store.mark_delivered(id).await
    }
}

#[derive(Debug, Deserialize)]
pub struct CreateMatchRequest {
    pub sport: SportId,
    pub wager_id: String,
    pub side_a: String,
    pub side_b: String,
    pub kind: MatchKind,
    pub series_rule: Option<SeriesRule>,
    pub series_n: Option<u32>,
    pub leg_configs: Vec<Value>,
    pub deadlines: Option<DeadlinePolicy>,
    pub abandon: Option<super::policy::AbandonPolicy>,
    pub tournament_id: Option<TournamentId>,
    pub match_id: Option<MatchId>,
    #[serde(default = "default_true")]
    pub auto_start: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Deserialize)]
pub struct CreateTournamentRequest {
    pub sport: SportId,
    pub wager_id: String,
    pub match_kind: MatchKind,
    pub round_kinds: Option<Vec<MatchKind>>,
    pub series_n: Option<u32>,
    pub series_rule: Option<SeriesRule>,
    pub leg_configs: Vec<Value>,
    pub entrants: Vec<String>,
    pub tournament_id: Option<TournamentId>,
}

#[derive(Debug, Serialize)]
pub struct MatchResponse {
    pub match_state: SeriesState,
    pub events: Vec<EngineEvent>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::engine::store::MemoryStore;
    use serde_json::json;

    #[tokio::test]
    async fn seed_spawns_ready_matches_and_spawn_leg_events() {
        let svc = EngineService::new(Arc::new(MemoryStore::default()));
        let t = svc
            .create_tournament(CreateTournamentRequest {
                sport: SportId::Lichess,
                wager_id: "0xabc".into(),
                match_kind: MatchKind::Single,
                round_kinds: None,
                series_n: None,
                series_rule: None,
                leg_configs: vec![json!({"timeControl": "10 mins | 0 secs"})],
                entrants: vec!["alice".into(), "bob".into(), "carol".into(), "dave".into()],
                tournament_id: None,
            })
            .await
            .unwrap();
        let (t, ready) = svc.seed_tournament(t.sport, t.id).await.unwrap();
        assert_eq!(ready.len(), 2);
        for mid in &ready {
            let m = svc.store.get_match(t.sport, *mid).await.unwrap();
            assert_eq!(m.tournament_id, Some(t.id));
            assert!(!m.legs.is_empty());
        }
        let pending = svc.pending_events(20).await.unwrap();
        let spawns = pending
            .iter()
            .filter(|r| matches!(r.event, EngineEvent::SpawnLeg { .. }))
            .count();
        assert_eq!(spawns, 2);
    }
}
