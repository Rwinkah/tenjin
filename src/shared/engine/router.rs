use axum::Router;
use axum::routing::{get, post};
use std::sync::Arc;

use crate::shared::engine::handler;
use crate::shared::models::AppState;

pub fn engine_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/engine/events", get(handler::list_events))
        .route("/engine/events/{id}/ack", post(handler::ack_event))
        .route("/engine/tick", post(handler::tick))
        .route("/engine/{sport}/matches", post(handler::create_match))
        .route("/engine/{sport}/matches/{match_id}", get(handler::get_match))
        .route(
            "/engine/{sport}/matches/by-wager/{wager_id}",
            get(handler::get_match_by_wager),
        )
        .route(
            "/engine/{sport}/matches/{match_id}/leg-created",
            post(handler::leg_created),
        )
        .route(
            "/engine/{sport}/matches/{match_id}/observe",
            post(handler::observe_leg),
        )
        .route(
            "/engine/{sport}/matches/{match_id}/deadline",
            post(handler::deadline_timeout),
        )
        .route(
            "/engine/{sport}/tournaments",
            post(handler::create_tournament),
        )
        .route(
            "/engine/{sport}/tournaments/{tournament_id}",
            get(handler::get_tournament),
        )
        .route(
            "/engine/{sport}/tournaments/{tournament_id}/register",
            post(handler::register_entrant),
        )
        .route(
            "/engine/{sport}/tournaments/{tournament_id}/seed",
            post(handler::seed_tournament),
        )
        .route(
            "/engine/{sport}/tournaments/{tournament_id}/ensure-matches",
            post(handler::ensure_ready_matches),
        )
        .route(
            "/engine/{sport}/tournaments/{tournament_id}/match-won",
            post(handler::tournament_match_won),
        )
        .route(
            "/engine/lichess/watch-leg",
            post(handler::lichess_watch_leg),
        )
}
