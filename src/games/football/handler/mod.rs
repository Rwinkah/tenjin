use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use alloy::primitives::{Address, B256, Bytes};
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};

use crate::games::football::api_football::{
    FixtureOutcomeClass, classify_fixture, fixture_status_label,
};
use crate::games::football::attestation::{
    build_binary_attestation, build_draw_attestation, build_join_permit, build_void_attestation,
};
use crate::games::football::fpl_fallback::resolve_fixture;
use crate::games::football::models::{FootballGameId, FootballMarketKind};
use crate::shared::min_stake::{self, GameKind};
use crate::shared::models::AppState;

const ATTESTATION_TTL_SECS: u64 = 7 * 24 * 3600;

pub async fn health_handler(State(app_state): State<Arc<AppState>>) -> String {
    format!(
        "football OK (signer: {}, oracle: {})",
        app_state.signer.address(),
        app_state.football_oracle_address
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

fn football_oracle(state: &AppState) -> Result<Address, (StatusCode, String)> {
    if state.football_oracle_address == Address::ZERO {
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            "FOOTBALL_ORACLE_ADDRESS is not configured".into(),
        ));
    }
    Ok(state.football_oracle_address)
}

// ---------- Join permit ----------

#[derive(Debug, Deserialize)]
pub struct JoinPermitRequest {
    pub wager_id: B256,
    pub joiner: Address,
    /// Exact on-chain gameId JSON string.
    pub game_id: String,
    /// Joiner's pick; must conflict with creatorPick in game_id.
    pub joiner_pick: String,
    /// Platform username (display_name) for directed wager checks.
    #[serde(default)]
    pub joiner_display_name: Option<String>,
    #[serde(default)]
    pub campaign: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct JoinPermitResponse {
    pub oracle: Address,
    pub deadline: u64,
    pub signature: Bytes,
}

/// `POST /football/attestations/join`
pub async fn join_attestation(
    State(state): State<Arc<AppState>>,
    Json(req): Json<JoinPermitRequest>,
) -> Result<Json<JoinPermitResponse>, (StatusCode, String)> {
    let meta = FootballGameId::parse(&req.game_id).map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    meta.ensure_conflicting_picks(&req.joiner_pick)
        .map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    meta.ensure_directed_joiner(req.joiner_display_name.as_deref())
        .map_err(|e| (StatusCode::FORBIDDEN, e))?;

    min_stake::enforce_min_stake(
        &state,
        req.campaign.as_deref(),
        GameKind::Football,
        req.wager_id,
    )
    .await?;

    // Soft check: fixture should still be pre-match when joining.
    let fixture = resolve_fixture(&state, &meta).await?;
    let class = classify_fixture(&fixture);
    if matches!(
        class,
        FixtureOutcomeClass::HomeWin
            | FixtureOutcomeClass::AwayWin
            | FixtureOutcomeClass::Draw
            | FixtureOutcomeClass::Void
    ) {
        return Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            format!(
                "fixture `{}` is no longer open for joining (status `{}`)",
                meta.fixture_id,
                fixture_status_label(&fixture)
            ),
        ));
    }

    let chain_id = signer_chain_id(&state)?;
    let oracle = football_oracle(&state)?;
    let deadline = now_deadline();

    let permit = build_join_permit(
        &state.signer,
        oracle,
        req.wager_id,
        req.joiner,
        chain_id,
        deadline,
    )
    .await
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to sign join permit: {e}"),
        )
    })?;

    Ok(Json(JoinPermitResponse {
        oracle: permit.oracle,
        deadline: permit.deadline,
        signature: permit.signature,
    }))
}

// ---------- Settlement ----------

#[derive(Debug, Deserialize)]
pub struct SettleAttestationRequest {
    pub wager_id: B256,
    /// Exact on-chain gameId JSON string.
    pub game_id: String,
    pub creator_address: Address,
    pub joiner_address: Address,
}

#[derive(Debug, Serialize)]
pub struct BinaryAttestationResponse {
    pub oracle: Address,
    pub data: Bytes,
    pub winner: Address,
    pub loser: Address,
    pub fixture_id: u64,
    pub status: String,
}

#[derive(Debug, Serialize)]
pub struct SimpleAttestationResponse {
    pub oracle: Address,
    pub data: Bytes,
    pub fixture_id: u64,
    pub status: String,
}

fn resolve_pick_addresses(
    creator_pick: &str,
    creator: Address,
    joiner_pick: &str,
    joiner: Address,
    winning_pick: &str,
) -> Result<(Address, Address), (StatusCode, String)> {
    if creator_pick == winning_pick {
        Ok((creator, joiner))
    } else if joiner_pick == winning_pick {
        Ok((joiner, creator))
    } else {
        Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            format!("neither pick matches winning side `{winning_pick}`"),
        ))
    }
}

fn resolved_picks(meta: &FootballGameId) -> Result<(String, String), (StatusCode, String)> {
    let joiner_pick = meta
        .joiner_pick
        .as_deref()
        .ok_or((StatusCode::BAD_REQUEST, "gameId missing joinerPick".into()))?;
    meta.ensure_conflicting_picks(joiner_pick)
        .map_err(|e| (StatusCode::BAD_REQUEST, e))
}

/// `POST /football/attestations/binary`
pub async fn binary_attestation(
    State(state): State<Arc<AppState>>,
    Json(req): Json<SettleAttestationRequest>,
) -> Result<Json<BinaryAttestationResponse>, (StatusCode, String)> {
    let meta = FootballGameId::parse(&req.game_id).map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    let (creator_pick, joiner_pick) = resolved_picks(&meta)?;
    let fixture = resolve_fixture(&state, &meta).await?;
    let class = classify_fixture(&fixture);

    let winning_pick = match (meta.kind, class) {
        (FootballMarketKind::HomeAway, FixtureOutcomeClass::HomeWin) => "1",
        (FootballMarketKind::HomeAway, FixtureOutcomeClass::AwayWin) => "2",
        (FootballMarketKind::HomeAway, FixtureOutcomeClass::Draw) => {
            return Err((
                StatusCode::UNPROCESSABLE_ENTITY,
                "match drew — use /attestations/draw".into(),
            ));
        }
        (FootballMarketKind::OverUnder, FixtureOutcomeClass::HomeWin)
        | (FootballMarketKind::OverUnder, FixtureOutcomeClass::AwayWin)
        | (FootballMarketKind::OverUnder, FixtureOutcomeClass::Draw) => {
            let home = fixture.goals.home.unwrap_or(0);
            let away = fixture.goals.away.unwrap_or(0);
            let total = home + away;
            let line = meta.line.unwrap_or(2.5);
            if (total as f64) > line { "O" } else { "U" }
        }
        (_, FixtureOutcomeClass::Void) => {
            return Err((
                StatusCode::UNPROCESSABLE_ENTITY,
                "fixture voided — use /attestations/void".into(),
            ));
        }
        (_, FixtureOutcomeClass::NotReady) => {
            return Err((
                StatusCode::UNPROCESSABLE_ENTITY,
                format!(
                    "fixture `{}` not ready to settle (status `{}`)",
                    meta.fixture_id,
                    fixture_status_label(&fixture)
                ),
            ));
        }
    };

    let (winner, loser) = resolve_pick_addresses(
        &creator_pick,
        req.creator_address,
        &joiner_pick,
        req.joiner_address,
        winning_pick,
    )?;

    let chain_id = signer_chain_id(&state)?;
    let oracle = football_oracle(&state)?;
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
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to sign attestation: {e}"),
        )
    })?;

    Ok(Json(BinaryAttestationResponse {
        oracle: attestation.oracle,
        data: attestation.data,
        winner,
        loser,
        fixture_id: meta.fixture_id,
        status: fixture.fixture.status.short,
    }))
}

/// `POST /football/attestations/draw` — Home/Away match draw push only.
pub async fn draw_attestation(
    State(state): State<Arc<AppState>>,
    Json(req): Json<SettleAttestationRequest>,
) -> Result<Json<SimpleAttestationResponse>, (StatusCode, String)> {
    let meta = FootballGameId::parse(&req.game_id).map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    if meta.kind != FootballMarketKind::HomeAway {
        return Err((
            StatusCode::BAD_REQUEST,
            "draw settlement only applies to football_home_away".into(),
        ));
    }
    let _ = resolved_picks(&meta)?;
    let fixture = resolve_fixture(&state, &meta).await?;
    if classify_fixture(&fixture) != FixtureOutcomeClass::Draw {
        return Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            format!(
                "fixture `{}` is not a draw (status `{}`, goals {:?}-{:?})",
                meta.fixture_id,
                fixture_status_label(&fixture),
                fixture.goals.home,
                fixture.goals.away
            ),
        ));
    }

    let chain_id = signer_chain_id(&state)?;
    let oracle = football_oracle(&state)?;
    let deadline = now_deadline();
    let attestation =
        build_draw_attestation(&state.signer, oracle, req.wager_id, chain_id, deadline)
            .await
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("failed to sign draw attestation: {e}"),
                )
            })?;

    Ok(Json(SimpleAttestationResponse {
        oracle: attestation.oracle,
        data: attestation.data,
        fixture_id: meta.fixture_id,
        status: fixture.fixture.status.short,
    }))
}

/// `POST /football/attestations/void`
pub async fn void_attestation(
    State(state): State<Arc<AppState>>,
    Json(req): Json<SettleAttestationRequest>,
) -> Result<Json<SimpleAttestationResponse>, (StatusCode, String)> {
    let meta = FootballGameId::parse(&req.game_id).map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    let _ = resolved_picks(&meta)?;
    let fixture = resolve_fixture(&state, &meta).await?;
    if classify_fixture(&fixture) != FixtureOutcomeClass::Void {
        return Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            format!(
                "fixture `{}` is not void (status `{}`)",
                meta.fixture_id,
                fixture_status_label(&fixture)
            ),
        ));
    }

    let chain_id = signer_chain_id(&state)?;
    let oracle = football_oracle(&state)?;
    let deadline = now_deadline();
    let attestation =
        build_void_attestation(&state.signer, oracle, req.wager_id, chain_id, deadline)
            .await
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("failed to sign void attestation: {e}"),
                )
            })?;

    Ok(Json(SimpleAttestationResponse {
        oracle: attestation.oracle,
        data: attestation.data,
        fixture_id: meta.fixture_id,
        status: fixture.fixture.status.short,
    }))
}
