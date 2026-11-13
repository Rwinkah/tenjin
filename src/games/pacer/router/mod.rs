use axum::Router;
use axum::middleware;
use axum::routing::{get, post};
use std::sync::Arc;

use crate::games::pacer::handler;
use crate::shared::create;
use crate::shared::models::AppState;
use crate::shared::service_auth::require_service_key;

pub fn pacer_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/health", get(handler::health_handler))
        .route("/attestations/binary", post(handler::binary_attestation))
        .route("/attestations/draw", post(handler::draw_attestation))
        .route("/attestations/void", post(handler::void_attestation))
        .merge(
            // Permits move funds on-chain: wager-backend only, after its passkey check.
            Router::new()
                .route("/attestations/join", post(handler::join_attestation))
                .route("/attestations/leave", post(handler::leave_attestation))
                .route("/attestations/create", post(create::pacer_create_attestation))
                .route_layer(middleware::from_fn(require_service_key)),
        )
}
