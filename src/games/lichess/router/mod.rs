use axum::routing::{get, post};
use axum::Router;
use std::sync::Arc;

use crate::games::lichess::handler;
use crate::shared::models::AppState;

/// Routes for this game only (paths are relative to where the parent nests this router).
pub fn lichess_router() -> Router<Arc<AppState>> {
    let r = Router::new();
    let r = r.route("/health", get(handler::health_handler));
    let r = r.route(
        "/attestations/binary",
        post(handler::binary_attestation),
    );
    let r = r.route(
        "/attestations/draw",
        post(handler::draw_attestation),
    );
    let r = r.route(
        "/attestations/void",
        post(handler::void_attestation),
    );
    let r = r.route(
        "/attestations/competition",
        post(handler::competition_attestation),
    );
    let r = r.route(
        "/attestations/competition/prepare-settle",
        post(handler::competition_prepare_settle),
    );
    let r = r.route(
        "/attestations/join",
        post(handler::join_attestation),
    );
    let r = r.route(
        "/attestations/competition-join",
        post(handler::competition_join_attestation),
    );
    let r = r.route(
        "/attestations/competition-void",
        post(handler::competition_void_attestation),
    );
    r
}


