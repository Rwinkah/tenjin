use std::sync::Arc;

use axum::extract::State;

use crate::shared::models::AppState;

pub async fn hello(State(state): State<Arc<AppState>>) -> String {
    format!(
        "tenjin OK (signer: {}, chain_id: {:?})",
        state.signer.address(),
        state.signer.chain_id()
    )
}
