use axum::{routing::get, Router};
use std::sync::Arc;
use crate::shared::models::AppState;
use crate::root::handler;

pub fn root_router() -> Router<Arc<AppState>> {
    let r = Router::new().route("/", get(handler::hello));
    r
}