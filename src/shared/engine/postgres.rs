//! Postgres-backed engine store (JSON documents + outbox).

use async_trait::async_trait;
use chrono::Utc;
use sqlx::PgPool;
use uuid::Uuid;

use super::events::EngineEvent;
use super::series::SeriesState;
use super::store::{EngineStore, OutboxRow, StoreError};
use super::tournament::TournamentState;
use super::types::{MatchId, SportId, TournamentId};

pub struct PostgresStore {
    pool: PgPool,
}

impl PostgresStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub fn pool_clone(&self) -> PgPool {
        self.pool.clone()
    }

    /// Create tables if missing (idempotent).
    pub async fn migrate(&self) -> Result<(), StoreError> {
        let stmts = [
            r#"
            CREATE TABLE IF NOT EXISTS engine_matches (
                sport TEXT NOT NULL,
                match_id UUID NOT NULL,
                wager_id TEXT NOT NULL,
                state JSONB NOT NULL,
                updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                PRIMARY KEY (sport, match_id)
            )
            "#,
            r#"
            CREATE UNIQUE INDEX IF NOT EXISTS engine_matches_wager_uidx
                ON engine_matches (sport, wager_id)
            "#,
            r#"
            CREATE TABLE IF NOT EXISTS engine_tournaments (
                sport TEXT NOT NULL,
                tournament_id UUID NOT NULL,
                wager_id TEXT NOT NULL,
                state JSONB NOT NULL,
                updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                PRIMARY KEY (sport, tournament_id)
            )
            "#,
            r#"
            CREATE TABLE IF NOT EXISTS engine_outbox (
                id UUID PRIMARY KEY,
                sport TEXT NOT NULL,
                created_at TIMESTAMPTZ NOT NULL,
                delivered BOOLEAN NOT NULL DEFAULT FALSE,
                event JSONB NOT NULL
            )
            "#,
            r#"
            CREATE INDEX IF NOT EXISTS engine_outbox_pending_idx
                ON engine_outbox (delivered, created_at)
                WHERE delivered = FALSE
            "#,
        ];
        for sql in stmts {
            sqlx::query(sql)
                .execute(&self.pool)
                .await
                .map_err(|e| StoreError::Message(e.to_string()))?;
        }
        Ok(())
    }
}

#[async_trait]
impl EngineStore for PostgresStore {
    async fn put_match(&self, state: &SeriesState) -> Result<(), StoreError> {
        let sport = state.sport.as_str();
        let json = serde_json::to_value(state).map_err(|e| StoreError::Message(e.to_string()))?;
        sqlx::query(
            r#"
            INSERT INTO engine_matches (sport, match_id, wager_id, state, updated_at)
            VALUES ($1, $2, $3, $4, NOW())
            ON CONFLICT (sport, match_id) DO UPDATE
              SET state = EXCLUDED.state,
                  wager_id = EXCLUDED.wager_id,
                  updated_at = NOW()
            "#,
        )
        .bind(sport)
        .bind(state.match_id.0)
        .bind(&state.wager_id)
        .bind(json)
        .execute(&self.pool)
        .await
        .map_err(|e| StoreError::Message(e.to_string()))?;
        Ok(())
    }

    async fn get_match(&self, sport: SportId, id: MatchId) -> Result<SeriesState, StoreError> {
        let row: Option<(serde_json::Value,)> = sqlx::query_as(
            "SELECT state FROM engine_matches WHERE sport = $1 AND match_id = $2",
        )
        .bind(sport.as_str())
        .bind(id.0)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| StoreError::Message(e.to_string()))?;
        let (state,) = row.ok_or(StoreError::NotFound)?;
        serde_json::from_value(state).map_err(|e| StoreError::Message(e.to_string()))
    }

    async fn get_match_by_wager(
        &self,
        sport: SportId,
        wager_id: &str,
    ) -> Result<SeriesState, StoreError> {
        let row: Option<(serde_json::Value,)> = sqlx::query_as(
            "SELECT state FROM engine_matches WHERE sport = $1 AND wager_id = $2",
        )
        .bind(sport.as_str())
        .bind(wager_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| StoreError::Message(e.to_string()))?;
        let (state,) = row.ok_or(StoreError::NotFound)?;
        serde_json::from_value(state).map_err(|e| StoreError::Message(e.to_string()))
    }

    async fn put_tournament(&self, state: &TournamentState) -> Result<(), StoreError> {
        let json = serde_json::to_value(state).map_err(|e| StoreError::Message(e.to_string()))?;
        sqlx::query(
            r#"
            INSERT INTO engine_tournaments (sport, tournament_id, wager_id, state, updated_at)
            VALUES ($1, $2, $3, $4, NOW())
            ON CONFLICT (sport, tournament_id) DO UPDATE
              SET state = EXCLUDED.state,
                  wager_id = EXCLUDED.wager_id,
                  updated_at = NOW()
            "#,
        )
        .bind(state.sport.as_str())
        .bind(state.id.0)
        .bind(&state.wager_id)
        .bind(json)
        .execute(&self.pool)
        .await
        .map_err(|e| StoreError::Message(e.to_string()))?;
        Ok(())
    }

    async fn get_tournament(
        &self,
        sport: SportId,
        id: TournamentId,
    ) -> Result<TournamentState, StoreError> {
        let row: Option<(serde_json::Value,)> = sqlx::query_as(
            "SELECT state FROM engine_tournaments WHERE sport = $1 AND tournament_id = $2",
        )
        .bind(sport.as_str())
        .bind(id.0)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| StoreError::Message(e.to_string()))?;
        let (state,) = row.ok_or(StoreError::NotFound)?;
        serde_json::from_value(state).map_err(|e| StoreError::Message(e.to_string()))
    }

    async fn push_events(&self, events: &[EngineEvent]) -> Result<(), StoreError> {
        let now = Utc::now();
        for event in events {
            let sport = match event {
                EngineEvent::SpawnLeg { sport, .. }
                | EngineEvent::LegTerminal { sport, .. }
                | EngineEvent::MatchTerminal { sport, .. }
                | EngineEvent::AdvanceBracket { sport, .. }
                | EngineEvent::TournamentComplete { sport, .. }
                | EngineEvent::Notify { sport, .. }
                | EngineEvent::ReadyToSettle { sport, .. } => *sport,
            };
            let json = serde_json::to_value(event).map_err(|e| StoreError::Message(e.to_string()))?;
            sqlx::query(
                r#"
                INSERT INTO engine_outbox (id, sport, created_at, delivered, event)
                VALUES ($1, $2, $3, FALSE, $4)
                "#,
            )
            .bind(Uuid::new_v4())
            .bind(sport.as_str())
            .bind(now)
            .bind(json)
            .execute(&self.pool)
            .await
            .map_err(|e| StoreError::Message(e.to_string()))?;
        }
        Ok(())
    }

    async fn list_pending_events(&self, limit: usize) -> Result<Vec<OutboxRow>, StoreError> {
        let rows: Vec<(Uuid, String, chrono::DateTime<Utc>, bool, serde_json::Value)> =
            sqlx::query_as(
                r#"
                SELECT id, sport, created_at, delivered, event
                FROM engine_outbox
                WHERE delivered = FALSE
                ORDER BY created_at ASC
                LIMIT $1
                "#,
            )
            .bind(limit as i64)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| StoreError::Message(e.to_string()))?;

        let mut out = Vec::new();
        for (id, sport, created_at, delivered, event) in rows {
            let sport = SportId::parse(&sport)
                .ok_or_else(|| StoreError::Message(format!("bad sport {sport}")))?;
            let event: EngineEvent =
                serde_json::from_value(event).map_err(|e| StoreError::Message(e.to_string()))?;
            out.push(OutboxRow {
                id,
                sport,
                created_at,
                delivered,
                event,
            });
        }
        Ok(out)
    }

    async fn mark_delivered(&self, id: Uuid) -> Result<(), StoreError> {
        sqlx::query("UPDATE engine_outbox SET delivered = TRUE WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| StoreError::Message(e.to_string()))?;
        Ok(())
    }
}
