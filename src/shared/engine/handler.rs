//! HTTP handlers for `/engine/{sport}/…`.

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;
use uuid::Uuid;

use crate::games::lichess::engine::LichessOutcomeClassifier;
use crate::shared::engine::events::Fault;
use crate::shared::engine::service::{
    CreateMatchRequest, CreateTournamentRequest, EngineService, MatchResponse,
};
use crate::shared::engine::store::StoreError;
use crate::shared::engine::types::{LegId, MatchId, SportId, TournamentId};
use crate::shared::engine::{LegOutcome, Side};
use crate::shared::models::AppState;

fn parse_sport(s: &str) -> Result<SportId, (StatusCode, String)> {
    SportId::parse(s).ok_or((StatusCode::BAD_REQUEST, format!("unknown sport `{s}`")))
}

fn map_err(e: StoreError) -> (StatusCode, String) {
    match e {
        StoreError::NotFound => (StatusCode::NOT_FOUND, "not found".into()),
        StoreError::NoDatabase => (StatusCode::SERVICE_UNAVAILABLE, "database required".into()),
        StoreError::Message(m) => (StatusCode::BAD_REQUEST, m),
    }
}

fn engine(state: &AppState) -> Result<&EngineService, (StatusCode, String)> {
    state
        .engine
        .as_ref()
        .ok_or((StatusCode::SERVICE_UNAVAILABLE, "engine not configured".into()))
}

#[derive(Debug, Deserialize)]
pub struct CreateMatchBody {
    pub wager_id: String,
    pub side_a: String,
    pub side_b: String,
    pub kind: crate::shared::engine::MatchKind,
    pub series_rule: Option<crate::shared::engine::SeriesRule>,
    pub series_n: Option<u32>,
    #[serde(default)]
    pub leg_configs: Vec<Value>,
    pub deadlines: Option<crate::shared::engine::DeadlinePolicy>,
    pub abandon: Option<crate::shared::engine::AbandonPolicy>,
    pub tournament_id: Option<Uuid>,
    pub match_id: Option<Uuid>,
    #[serde(default = "default_true")]
    pub auto_start: bool,
}

fn default_true() -> bool {
    true
}

pub async fn create_match(
    State(state): State<Arc<AppState>>,
    Path(sport): Path<String>,
    Json(body): Json<CreateMatchBody>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let sport = parse_sport(&sport)?;
    let svc = engine(&state)?;
    let req = CreateMatchRequest {
        sport,
        wager_id: body.wager_id,
        side_a: body.side_a,
        side_b: body.side_b,
        kind: body.kind,
        series_rule: body.series_rule,
        series_n: body.series_n,
        leg_configs: body.leg_configs,
        deadlines: body.deadlines.map(|d| d.clamped()),
        abandon: body.abandon,
        tournament_id: body.tournament_id.map(TournamentId),
        match_id: body.match_id.map(MatchId),
        auto_start: body.auto_start,
    };
    let m = svc.create_match(req).await.map_err(map_err)?;
    Ok(Json(json!({ "match": m })))
}

pub async fn get_match(
    State(state): State<Arc<AppState>>,
    Path((sport, match_id)): Path<(String, Uuid)>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let sport = parse_sport(&sport)?;
    let svc = engine(&state)?;
    let m = svc
        .store()
        .get_match(sport, MatchId(match_id))
        .await
        .map_err(map_err)?;
    Ok(Json(json!({ "match": m })))
}

pub async fn get_match_by_wager(
    State(state): State<Arc<AppState>>,
    Path((sport, wager_id)): Path<(String, String)>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let sport = parse_sport(&sport)?;
    let svc = engine(&state)?;
    let m = svc
        .store()
        .get_match_by_wager(sport, &wager_id)
        .await
        .map_err(map_err)?;
    Ok(Json(json!({ "match": m })))
}

#[derive(Debug, Deserialize)]
pub struct LegCreatedBody {
    pub leg_id: Uuid,
    pub external_ref: String,
}

pub async fn leg_created(
    State(state): State<Arc<AppState>>,
    Path((sport, match_id)): Path<(String, Uuid)>,
    Json(body): Json<LegCreatedBody>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let sport = parse_sport(&sport)?;
    let svc = engine(&state)?;
    let m = svc
        .leg_created(sport, MatchId(match_id), LegId(body.leg_id), body.external_ref)
        .await
        .map_err(map_err)?;
    Ok(Json(json!({ "match": m })))
}

#[derive(Debug, Deserialize)]
pub struct ObserveBody {
    pub leg_id: Uuid,
    /// Direct outcome override (tests / executor).
    pub outcome: Option<ObserveOutcome>,
    /// Raw sport payload (e.g. Lichess export JSON).
    pub raw: Option<Value>,
    pub side_a_username: Option<String>,
    pub side_b_username: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ObserveOutcome {
    Ongoing,
    Decisive { winner: Side },
    Draw,
    Forfeit { loser: Side },
    MutualVoid,
    FaultVoid,
}

impl From<ObserveOutcome> for LegOutcome {
    fn from(o: ObserveOutcome) -> Self {
        match o {
            ObserveOutcome::Ongoing => LegOutcome::Ongoing,
            ObserveOutcome::Decisive { winner } => LegOutcome::Decisive { winner },
            ObserveOutcome::Draw => LegOutcome::Draw,
            ObserveOutcome::Forfeit { loser } => LegOutcome::Forfeit { loser },
            ObserveOutcome::MutualVoid => LegOutcome::MutualVoid,
            ObserveOutcome::FaultVoid => LegOutcome::FaultVoid,
        }
    }
}

pub async fn observe_leg(
    State(state): State<Arc<AppState>>,
    Path((sport, match_id)): Path<(String, Uuid)>,
    Json(body): Json<ObserveBody>,
) -> Result<Json<MatchResponse>, (StatusCode, String)> {
    let sport = parse_sport(&sport)?;
    let svc = engine(&state)?;
    let (m, events) = if let Some(o) = body.outcome {
        svc.observe_leg(sport, MatchId(match_id), LegId(body.leg_id), o.into())
            .await
            .map_err(map_err)?
    } else if let Some(raw) = body.raw {
        if sport != SportId::Lichess {
            return Err((
                StatusCode::BAD_REQUEST,
                "raw observe currently supported for lichess only".into(),
            ));
        }
        let a = body.side_a_username.unwrap_or_default();
        let b = body.side_b_username.unwrap_or_default();
        let classifier = LichessOutcomeClassifier::new(a, b);
        svc.observe_leg_raw(
            sport,
            MatchId(match_id),
            LegId(body.leg_id),
            &raw,
            &classifier,
        )
        .await
        .map_err(map_err)?
    } else {
        return Err((StatusCode::BAD_REQUEST, "outcome or raw required".into()));
    };
    Ok(Json(MatchResponse {
        match_state: m,
        events,
    }))
}

#[derive(Debug, Deserialize)]
pub struct DeadlineBody {
    pub leg_id: Uuid,
    pub fault: Fault,
}

pub async fn deadline_timeout(
    State(state): State<Arc<AppState>>,
    Path((sport, match_id)): Path<(String, Uuid)>,
    Json(body): Json<DeadlineBody>,
) -> Result<Json<MatchResponse>, (StatusCode, String)> {
    let sport = parse_sport(&sport)?;
    let svc = engine(&state)?;
    let (m, events) = svc
        .deadline_timeout(sport, MatchId(match_id), LegId(body.leg_id), body.fault)
        .await
        .map_err(map_err)?;
    Ok(Json(MatchResponse {
        match_state: m,
        events,
    }))
}

#[derive(Debug, Deserialize)]
pub struct CreateTournamentBody {
    pub wager_id: String,
    pub match_kind: crate::shared::engine::MatchKind,
    /// Optional per-round kinds (`single` | `series`). Index 0 = first round.
    #[serde(default)]
    pub round_kinds: Option<Vec<crate::shared::engine::MatchKind>>,
    pub series_n: Option<u32>,
    pub series_rule: Option<crate::shared::engine::SeriesRule>,
    #[serde(default)]
    pub leg_configs: Vec<Value>,
    #[serde(default)]
    pub entrants: Vec<String>,
    pub tournament_id: Option<Uuid>,
}

pub async fn create_tournament(
    State(state): State<Arc<AppState>>,
    Path(sport): Path<String>,
    Json(body): Json<CreateTournamentBody>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let sport = parse_sport(&sport)?;
    let svc = engine(&state)?;
    let t = svc
        .create_tournament(CreateTournamentRequest {
            sport,
            wager_id: body.wager_id,
            match_kind: body.match_kind,
            round_kinds: body.round_kinds,
            series_n: body.series_n,
            series_rule: body.series_rule,
            leg_configs: body.leg_configs,
            entrants: body.entrants,
            tournament_id: body.tournament_id.map(TournamentId),
        })
        .await
        .map_err(map_err)?;
    Ok(Json(json!({ "tournament": t })))
}

pub async fn get_tournament(
    State(state): State<Arc<AppState>>,
    Path((sport, tournament_id)): Path<(String, Uuid)>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let sport = parse_sport(&sport)?;
    let svc = engine(&state)?;
    let t = svc
        .store()
        .get_tournament(sport, TournamentId(tournament_id))
        .await
        .map_err(map_err)?;
    Ok(Json(json!({ "tournament": t })))
}

#[derive(Debug, Deserialize)]
pub struct RegisterBody {
    pub entrant: String,
}

pub async fn register_entrant(
    State(state): State<Arc<AppState>>,
    Path((sport, tournament_id)): Path<(String, Uuid)>,
    Json(body): Json<RegisterBody>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let sport = parse_sport(&sport)?;
    let svc = engine(&state)?;
    let t = svc
        .register_entrant(sport, TournamentId(tournament_id), body.entrant)
        .await
        .map_err(map_err)?;
    Ok(Json(json!({ "tournament": t })))
}

pub async fn seed_tournament(
    State(state): State<Arc<AppState>>,
    Path((sport, tournament_id)): Path<(String, Uuid)>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let sport = parse_sport(&sport)?;
    let svc = engine(&state)?;
    let (t, ready) = svc
        .seed_tournament(sport, TournamentId(tournament_id))
        .await
        .map_err(map_err)?;
    Ok(Json(json!({ "tournament": t, "ready_match_ids": ready })))
}

pub async fn ensure_ready_matches(
    State(state): State<Arc<AppState>>,
    Path((sport, tournament_id)): Path<(String, Uuid)>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let sport = parse_sport(&sport)?;
    let svc = engine(&state)?;
    let (t, ready) = svc
        .ensure_ready_matches(sport, TournamentId(tournament_id))
        .await
        .map_err(map_err)?;
    Ok(Json(json!({ "tournament": t, "ready_match_ids": ready })))
}

#[derive(Debug, Deserialize)]
pub struct MatchWonBody {
    pub from_match_id: Uuid,
    pub winner: String,
}

pub async fn tournament_match_won(
    State(state): State<Arc<AppState>>,
    Path((sport, tournament_id)): Path<(String, Uuid)>,
    Json(body): Json<MatchWonBody>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let sport = parse_sport(&sport)?;
    let svc = engine(&state)?;
    let (t, events, next) = svc
        .tournament_match_won(
            sport,
            TournamentId(tournament_id),
            MatchId(body.from_match_id),
            body.winner,
        )
        .await
        .map_err(map_err)?;
    Ok(Json(json!({
        "tournament": t,
        "events": events,
        "next_match_id": next,
    })))
}

#[derive(Debug, Deserialize)]
pub struct EventsQuery {
    #[serde(default = "default_limit")]
    pub limit: usize,
}

fn default_limit() -> usize {
    50
}

pub async fn list_events(
    State(state): State<Arc<AppState>>,
    Query(q): Query<EventsQuery>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let svc = engine(&state)?;
    let rows = svc.pending_events(q.limit).await.map_err(map_err)?;
    Ok(Json(json!({ "events": rows })))
}

pub async fn ack_event(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let svc = engine(&state)?;
    svc.ack_event(id).await.map_err(map_err)?;
    Ok(Json(json!({ "ok": true })))
}

/// Enforce accept/start deadlines, then return pending outbox events for executors.
pub async fn tick(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let svc = engine(&state)?;
    let (timed_out, _) = svc
        .enforce_accept_deadlines(crate::shared::engine::types::SportId::Lichess, 100)
        .await
        .map_err(map_err)?;
    let pending = svc.pending_events(100).await.map_err(map_err)?;
    Ok(Json(json!({
        "accept_deadlines_enforced": timed_out,
        "pending_events": pending.len(),
        "events": pending,
    })))
}

pub async fn lichess_watch_leg(
    State(state): State<Arc<AppState>>,
    Json(body): Json<crate::games::lichess::engine::watcher::WatchLegRequest>,
) -> Result<Json<MatchResponse>, (StatusCode, String)> {
    let svc = engine(&state)?;
    let (m, events) =
        crate::games::lichess::engine::watcher::watch_and_observe(&state.http, svc, body).await?;
    Ok(Json(MatchResponse {
        match_state: m,
        events,
    }))
}
