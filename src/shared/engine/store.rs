//! Persistence for engine state + outbox events.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;
use thiserror::Error;
use uuid::Uuid;

use super::events::EngineEvent;
use super::series::SeriesState;
use super::tournament::TournamentState;
use super::types::{MatchId, SportId, TournamentId};

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("{0}")]
    Message(String),
    #[error("not found")]
    NotFound,
    #[error("database required")]
    NoDatabase,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutboxRow {
    pub id: Uuid,
    pub sport: SportId,
    pub created_at: DateTime<Utc>,
    pub delivered: bool,
    pub event: EngineEvent,
}

#[async_trait]
pub trait EngineStore: Send + Sync {
    async fn put_match(&self, state: &SeriesState) -> Result<(), StoreError>;
    async fn get_match(&self, sport: SportId, id: MatchId) -> Result<SeriesState, StoreError>;
    async fn get_match_by_wager(
        &self,
        sport: SportId,
        wager_id: &str,
    ) -> Result<SeriesState, StoreError>;

    /// Non-terminal matches (for deadline enforcement). Best-effort, capped.
    async fn list_open_matches(
        &self,
        sport: SportId,
        limit: usize,
    ) -> Result<Vec<SeriesState>, StoreError>;

    async fn put_tournament(&self, state: &TournamentState) -> Result<(), StoreError>;
    async fn get_tournament(
        &self,
        sport: SportId,
        id: TournamentId,
    ) -> Result<TournamentState, StoreError>;

    async fn push_events(&self, events: &[EngineEvent]) -> Result<(), StoreError>;
    async fn list_pending_events(&self, limit: usize) -> Result<Vec<OutboxRow>, StoreError>;
    async fn mark_delivered(&self, id: Uuid) -> Result<(), StoreError>;
}

/// In-memory store for tests and when DATABASE_URL is unset.
#[derive(Default)]
pub struct MemoryStore {
    matches: Mutex<HashMap<(SportId, Uuid), SeriesState>>,
    by_wager: Mutex<HashMap<(SportId, String), Uuid>>,
    tournaments: Mutex<HashMap<(SportId, Uuid), TournamentState>>,
    outbox: Mutex<Vec<OutboxRow>>,
}

#[async_trait]
impl EngineStore for MemoryStore {
    async fn put_match(&self, state: &SeriesState) -> Result<(), StoreError> {
        let id = state.match_id.0;
        self.matches
            .lock()
            .unwrap()
            .insert((state.sport, id), state.clone());
        self.by_wager
            .lock()
            .unwrap()
            .insert((state.sport, state.wager_id.clone()), id);
        Ok(())
    }

    async fn get_match(&self, sport: SportId, id: MatchId) -> Result<SeriesState, StoreError> {
        self.matches
            .lock()
            .unwrap()
            .get(&(sport, id.0))
            .cloned()
            .ok_or(StoreError::NotFound)
    }

    async fn get_match_by_wager(
        &self,
        sport: SportId,
        wager_id: &str,
    ) -> Result<SeriesState, StoreError> {
        let id = self
            .by_wager
            .lock()
            .unwrap()
            .get(&(sport, wager_id.to_string()))
            .copied()
            .ok_or(StoreError::NotFound)?;
        self.get_match(sport, MatchId(id)).await
    }

    async fn list_open_matches(
        &self,
        sport: SportId,
        limit: usize,
    ) -> Result<Vec<SeriesState>, StoreError> {
        let guard = self.matches.lock().unwrap();
        let mut out: Vec<SeriesState> = guard
            .values()
            .filter(|m| m.sport == sport && !m.phase.is_terminal())
            .cloned()
            .collect();
        out.truncate(limit);
        Ok(out)
    }

    async fn put_tournament(&self, state: &TournamentState) -> Result<(), StoreError> {
        self.tournaments
            .lock()
            .unwrap()
            .insert((state.sport, state.id.0), state.clone());
        Ok(())
    }

    async fn get_tournament(
        &self,
        sport: SportId,
        id: TournamentId,
    ) -> Result<TournamentState, StoreError> {
        self.tournaments
            .lock()
            .unwrap()
            .get(&(sport, id.0))
            .cloned()
            .ok_or(StoreError::NotFound)
    }

    async fn push_events(&self, events: &[EngineEvent]) -> Result<(), StoreError> {
        let now = Utc::now();
        let mut box_ = self.outbox.lock().unwrap();
        for event in events {
            box_.push(OutboxRow {
                id: Uuid::new_v4(),
                sport: match event {
                    EngineEvent::SpawnLeg { sport, .. }
                    | EngineEvent::LegTerminal { sport, .. }
                    | EngineEvent::MatchTerminal { sport, .. }
                    | EngineEvent::AdvanceBracket { sport, .. }
                    | EngineEvent::TournamentComplete { sport, .. }
                    | EngineEvent::Notify { sport, .. }
                    | EngineEvent::ReadyToSettle { sport, .. } => *sport,
                },
                created_at: now,
                delivered: false,
                event: event.clone(),
            });
        }
        Ok(())
    }

    async fn list_pending_events(&self, limit: usize) -> Result<Vec<OutboxRow>, StoreError> {
        Ok(self
            .outbox
            .lock()
            .unwrap()
            .iter()
            .filter(|r| !r.delivered)
            .take(limit)
            .cloned()
            .collect())
    }

    async fn mark_delivered(&self, id: Uuid) -> Result<(), StoreError> {
        if let Some(row) = self.outbox.lock().unwrap().iter_mut().find(|r| r.id == id) {
            row.delivered = true;
        }
        Ok(())
    }
}
