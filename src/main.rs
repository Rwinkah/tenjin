mod games;
mod root;
mod shared;

use axum::Router;
use std::net::SocketAddr;
use std::sync::Arc;
use tower_http::cors::{Any, CorsLayer};

use games::football::router::football_router;
use games::lichess::router::lichess_router;
use root::router::root_router;
use shared::models::AppState;

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();

    let state = Arc::new(AppState::from_env());

    let host = std::env::var("HOST")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "0.0.0.0".to_string());

    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or(3001);

    let addr: SocketAddr = format!("{host}:{port}")
        .parse()
        .expect("HOST and PORT must form a valid socket address");

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let app = Router::new()
        .merge(root_router())
        .merge(lichess_router())
        .nest("/football", football_router())
        .layer(cors)
        .with_state(state);

    println!("tenjin listening on http://{addr}");
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
