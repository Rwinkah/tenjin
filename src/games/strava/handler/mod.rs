use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use alloy::primitives::{Address, Bytes, B256};
use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::games::strava::attestation::{
    build_binary_attestation, build_draw_attestation, build_join_permit, build_void_attestation,
};
use crate::games::strava::models::StravaGameId;
use crate::shared::min_stake::{self, GameKind};
use crate::shared::models::AppState;

const ATTESTATION_TTL_SECS: u64 = 7 * 24 * 3600;

pub async fn health_handler(State(app_state): State<Arc<AppState>>) -> String {
    format!(
        "strava OK (signer: {}, oracle: {})",
        app_state.signer.address(),
        app_state.strava_oracle_address
    )
}

fn now_deadline() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() + ATTESTATION_TTL_SECS)
        .unwrap_or(ATTESTATION_TTL_SECS)
}

fn signer_chain_id(state: &AppState) -> Result<u64, (StatusCode, String)> {
    state.signer.chain_id().ok_or((
        StatusCode::INTERNAL_SERVER_ERROR,
        "signer has no chain_id configured".into(),
    ))
}

fn strava_oracle(state: &AppState) -> Result<Address, (StatusCode, String)> {
    if state.strava_oracle_address == Address::ZERO {
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            "STRAVA_ORACLE_ADDRESS is not configured".into(),
        ));
    }
    Ok(state.strava_oracle_address)
}

fn ensure_chain_id(
    state: &AppState,
    requested: Option<u64>,
) -> Result<u64, (StatusCode, String)> {
    let configured = signer_chain_id(state)?;
    if let Some(id) = requested {
        if id != configured {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("chain_id {id} does not match signer chain {configured}"),
            ));
        }
    }
    Ok(configured)
}

fn reject_zero(addr: Address, label: &str) -> Result<Address, (StatusCode, String)> {
    if addr == Address::ZERO {
        return Err((
            StatusCode::BAD_REQUEST,
            format!("{label} must be a non-zero address"),
        ));
    }
    Ok(addr)
}

fn sign_error(ctx: &str, err: alloy::signers::Error) -> (StatusCode, String) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        format!("failed to sign {ctx}: {err}"),
    )
}

// ---------- Join permit ----------

#[derive(Debug, Deserialize)]
pub struct JoinPermitRequest {
    pub wager_id: B256,
    pub joiner: Address,
    #[serde(default)]
    pub game_id: String,
    #[serde(default)]
    pub joiner_display_name: Option<String>,
    #[serde(default)]
    pub chain_id: Option<u64>,
    #[serde(default)]
    pub campaign: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct JoinPermitResponse {
    pub oracle: Address,
    pub deadline: u64,
    pub signature: Bytes,
}

/// `POST /strava/attestations/join`
pub async fn join_attestation(
    State(state): State<Arc<AppState>>,
    Json(req): Json<JoinPermitRequest>,
) -> Result<Json<JoinPermitResponse>, (StatusCode, String)> {
    let joiner = reject_zero(req.joiner, "joiner")?;
    min_stake::enforce_min_stake(&state, req.campaign.as_deref(), GameKind::Strava, req.wager_id)
        .await?;
    if !req.game_id.trim().is_empty() {
        let meta = StravaGameId::parse(&req.game_id).map_err(|e| (StatusCode::BAD_REQUEST, e))?;
        meta.ensure_directed_joiner(req.joiner_display_name.as_deref())
            .map_err(|e| (StatusCode::FORBIDDEN, e))?;
    }

    let chain_id = ensure_chain_id(&state, req.chain_id)?;
    let oracle = strava_oracle(&state)?;
    let deadline = now_deadline();
    let permit = build_join_permit(
        &state.signer,
        oracle,
        req.wager_id,
        joiner,
        chain_id,
        deadline,
    )
    .await
    .map_err(|e| sign_error("join permit", e))?;

    Ok(Json(JoinPermitResponse {
        oracle: permit.oracle,
        deadline: permit.deadline,
        signature: permit.signature,
    }))
}

// ---------- Settlement ----------

#[derive(Debug, Deserialize)]
pub struct BinaryAttestationRequest {
    pub wager_id: B256,
    pub winner: Address,
    pub loser: Address,
    #[serde(default)]
    pub creator_activity_id: String,
    #[serde(default)]
    pub joiner_activity_id: String,
    #[serde(default)]
    pub chain_id: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct BinaryAttestationResponse {
    pub oracle: Address,
    pub data: Bytes,
    pub winner: Address,
    pub loser: Address,
    pub creator_activity_id: String,
    pub joiner_activity_id: String,
}

/// `POST /strava/attestations/binary`
///
/// Django already verified Strava activities (OAuth tokens live there) and
/// computed winner/loser. Tenjin signs that outcome for `StravaOracle`.
pub async fn binary_attestation(
    State(state): State<Arc<AppState>>,
    Json(req): Json<BinaryAttestationRequest>,
) -> Result<Json<BinaryAttestationResponse>, (StatusCode, String)> {
    let winner = reject_zero(req.winner, "winner")?;
    let loser = reject_zero(req.loser, "loser")?;
    if winner == loser {
        return Err((
            StatusCode::BAD_REQUEST,
            "winner and loser must be different".into(),
        ));
    }

    let chain_id = ensure_chain_id(&state, req.chain_id)?;
    let oracle = strava_oracle(&state)?;
    let deadline = now_deadline();
    let attestation = build_binary_attestation(
        &state.signer,
        oracle,
        req.wager_id,
        winner,
        loser,
        chain_id,
        deadline,
    )
    .await
    .map_err(|e| sign_error("attestation", e))?;

    Ok(Json(BinaryAttestationResponse {
        oracle: attestation.oracle,
        data: attestation.data,
        winner,
        loser,
        creator_activity_id: req.creator_activity_id,
        joiner_activity_id: req.joiner_activity_id,
    }))
}

#[derive(Debug, Deserialize)]
pub struct DrawAttestationRequest {
    pub wager_id: B256,
    #[serde(default)]
    pub creator_activity_id: String,
    #[serde(default)]
    pub joiner_activity_id: String,
    #[serde(default)]
    pub chain_id: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct SimpleAttestationResponse {
    pub oracle: Address,
    pub data: Bytes,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub creator_activity_id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub joiner_activity_id: String,
}

/// `POST /strava/attestations/draw`
pub async fn draw_attestation(
    State(state): State<Arc<AppState>>,
    Json(req): Json<DrawAttestationRequest>,
) -> Result<Json<SimpleAttestationResponse>, (StatusCode, String)> {
    let chain_id = ensure_chain_id(&state, req.chain_id)?;
    let oracle = strava_oracle(&state)?;
    let deadline = now_deadline();
    let attestation =
        build_draw_attestation(&state.signer, oracle, req.wager_id, chain_id, deadline)
            .await
            .map_err(|e| sign_error("draw attestation", e))?;

    Ok(Json(SimpleAttestationResponse {
        oracle: attestation.oracle,
        data: attestation.data,
        reason: None,
        creator_activity_id: req.creator_activity_id,
        joiner_activity_id: req.joiner_activity_id,
    }))
}

#[derive(Debug, Deserialize)]
pub struct VoidAttestationRequest {
    pub wager_id: B256,
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    pub chain_id: Option<u64>,
}

/// `POST /strava/attestations/void`
pub async fn void_attestation(
    State(state): State<Arc<AppState>>,
    Json(req): Json<VoidAttestationRequest>,
) -> Result<Json<SimpleAttestationResponse>, (StatusCode, String)> {
    let chain_id = ensure_chain_id(&state, req.chain_id)?;
    let oracle = strava_oracle(&state)?;
    let deadline = now_deadline();
    let attestation =
        build_void_attestation(&state.signer, oracle, req.wager_id, chain_id, deadline)
            .await
            .map_err(|e| sign_error("void attestation", e))?;

    Ok(Json(SimpleAttestationResponse {
        oracle: attestation.oracle,
        data: attestation.data,
        reason: {
            let trimmed = req.reason.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        },
        creator_activity_id: String::new(),
        joiner_activity_id: String::new(),
    }))
}
