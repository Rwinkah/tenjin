use axum::Router;
use axum::middleware;
use axum::routing::{get, post};
use std::sync::Arc;

use crate::games::lichess::handler;
use crate::shared::create;
use crate::shared::models::AppState;
use crate::shared::service_auth::require_service_key;

/// Routes for this game only (paths are relative to where the parent nests this router).
pub fn lichess_router() -> Router<Arc<AppState>> {
    let public = Router::new()
        .route("/health", get(handler::health_handler))
        .route("/attestations/binary", post(handler::binary_attestation))
        .route("/attestations/draw", post(handler::draw_attestation))
        .route("/attestations/void", post(handler::void_attestation))
        .route("/attestations/competition", post(handler::competition_attestation))
        .route(
            "/attestations/competition/prepare-settle",
            post(handler::competition_prepare_settle),
        )
        .route(
            "/attestations/competition-void",
            post(handler::competition_void_attestation),
        );

    // Permits move funds on-chain: wager-backend only, after its passkey check.
    let permits = Router::new()
        .route("/attestations/join", post(handler::join_attestation))
        .route(
            "/attestations/competition-join",
            post(handler::competition_join_attestation),
        )
        .route("/attestations/leave", post(handler::leave_attestation))
        .route("/attestations/create", post(create::lichess_create_attestation))
        .route_layer(middleware::from_fn(require_service_key));

    public.merge(permits)
}
