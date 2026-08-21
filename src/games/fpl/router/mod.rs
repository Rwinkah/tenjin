use axum::routing::{get, post};
use axum::Router;
use std::sync::Arc;

use crate::games::fpl::handler;
use crate::shared::models::AppState;

pub fn fpl_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/health", get(handler::health_handler))
        .route("/attestations/join", post(handler::join_attestation))
        .route("/attestations/binary", post(handler::binary_attestation))
        .route("/attestations/draw", post(handler::draw_attestation))
        .route("/attestations/void", post(handler::void_attestation))
        .route(
            "/attestations/competition",
            post(handler::competition_attestation),
        )
        .route(
            "/attestations/competition-void",
            post(handler::competition_void_attestation),
        )
}
