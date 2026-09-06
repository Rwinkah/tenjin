use crate::root::handler;
use crate::shared::models::AppState;
use axum::{Router, routing::get};
use std::sync::Arc;

pub fn root_router() -> Router<Arc<AppState>> {
    let r = Router::new().route("/", get(handler::hello));
    r
}
