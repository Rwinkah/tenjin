//! High-level engine operations used by HTTP handlers.

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;

use super::events::{EngineEvent, Fault};
use super::leg::LegOutcome;
use super::policy::{DeadlinePolicy, SeriesConfig};
use super::series::SeriesState;
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
        let events = state
            .on_leg_observation(leg_id, outcome, Utc::now())
            .map_err(StoreError::Message)?;
        self.store.put_match(&state).await?;
        if !events.is_empty() {
            self.store.push_events(&events).await?;
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
        Ok((state, events, next))
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
