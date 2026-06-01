use std::sync::Arc;

use alloy::primitives::{Address, Bytes, B256, U256};
use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use reqwest::header::ACCEPT;
use serde::{Deserialize, Serialize};

use crate::games::lichess::attestation::{
    build_binary_attestation, build_competition_attestation, RankedPlacement,
};
use crate::games::lichess::models::{LichessColor, LichessGameExport, LichessStatus};
use crate::shared::models::AppState;

const LICHESS_GAME_EXPORT_URL: &str = "https://lichess.org/game/export";

pub async fn health_handler(State(app_state): State<Arc<AppState>>) -> String {
    format!(
        "lichess OK (signer: {}, oracle: {})",
        app_state.signer.address(),
        app_state.oracle_address
    )
}

// ---------- Binary (1v1) attestation ----------

#[derive(Debug, Deserialize)]
pub struct BinaryAttestationRequest {
    /// `bytes32` id of the wager being settled (must match `lcSettle` calldata).
    pub wager_id: B256,
    /// Lichess game id (8 chars). The oracle fetches this game and derives the winner.
    pub game_id: String,
    /// Address that played White in this Lichess game.
    pub white_address: Address,
    /// Address that played Black in this Lichess game.
    pub black_address: Address,
}

#[derive(Debug, Serialize)]
pub struct BinaryAttestationResponse {
    /// Address of the `LichessOracle` contract.
    pub oracle: Address,
    /// `abi.encode(uint256 deadline, bytes signature)` — pass into `OracleAttestation.data`.
    pub data: Bytes,

    pub winner: Address,
    pub loser: Address,
    pub winner_color: LichessColor,
    /// Lichess username of the winning player, if Lichess reported a logged-in user.
    /// Useful so the caller can sanity-check the address mapping they provided.
    pub winner_lichess_id: Option<String>,

    pub game_id: String,
    pub raw_status: LichessStatus,
}

/// `POST /lichess/attestations/binary`
///
/// Fetches the Lichess game, requires a decisive result, then returns a signed
/// `OracleAttestation` ready to be passed to `LichessContract.lcSettle`.
pub async fn binary_attestation(
    State(state): State<Arc<AppState>>,
    Json(req): Json<BinaryAttestationRequest>,
) -> Result<Json<BinaryAttestationResponse>, (StatusCode, String)> {
    validate_game_id(&req.game_id)?;

    let game = fetch_lichess_game(&state, &req.game_id).await?;

    let (winner, loser, winner_color, winner_lichess_id) = match (game.status, game.winner) {
        (
            LichessStatus::Mate
            | LichessStatus::Resign
            | LichessStatus::Timeout
            | LichessStatus::Outoftime
            | LichessStatus::Cheat
            | LichessStatus::VariantEnd,
            Some(color),
        ) => {
            let lichess_id = match color {
                LichessColor::White => game.players.white.user.as_ref().map(|u| u.id.clone()),
                LichessColor::Black => game.players.black.user.as_ref().map(|u| u.id.clone()),
            };
            let (winner, loser) = match color {
                LichessColor::White => (req.white_address, req.black_address),
                LichessColor::Black => (req.black_address, req.white_address),
            };
            (winner, loser, color, lichess_id)
        }
        _ => {
            return Err((
                StatusCode::UNPROCESSABLE_ENTITY,
                format!(
                    "game `{}` cannot be settled: status `{:?}`, winner `{:?}`",
                    game.id, game.status, game.winner
                ),
            ));
        }
    };

    let chain_id = signer_chain_id(&state)?;

    let attestation = build_binary_attestation(
        &state.signer,
        state.oracle_address,
        req.wager_id,
        winner,
        loser,
        chain_id,
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
        winner_color,
        winner_lichess_id,
        game_id: game.id,
        raw_status: game.status,
    }))
}

// ---------- Competition (ranked) attestation ----------

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct RankedPlacementInput {
    /// 1-based rank (must match `lcSettleCompetition` calldata).
    pub rank: u64,
    pub player: Address,
}

#[derive(Debug, Deserialize)]
pub struct CompetitionAttestationRequest {
    pub wager_id: B256,
    /// Ranked placements — order must match the array passed to `lcSettleCompetition`.
    pub placements: Vec<RankedPlacementInput>,
}

#[derive(Debug, Serialize)]
pub struct CompetitionAttestationResponse {
    pub oracle: Address,
    pub data: Bytes,
    pub placements: Vec<RankedPlacementInput>,
}

/// `POST /attestations/competition`
///
/// Signs a ranked placement attestation. Each entry is a `{rank, player}` pair
/// matching `RankedPlacement` on `LichessOracle`.
pub async fn competition_attestation(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CompetitionAttestationRequest>,
) -> Result<Json<CompetitionAttestationResponse>, (StatusCode, String)> {
    validate_placements(&req.placements)?;

    let placements: Vec<RankedPlacement> = req
        .placements
        .iter()
        .map(|entry| RankedPlacement {
            rank: U256::from(entry.rank),
            player: entry.player,
        })
        .collect();

    let chain_id = signer_chain_id(&state)?;

    let attestation = build_competition_attestation(
        &state.signer,
        state.oracle_address,
        req.wager_id,
        &placements,
        chain_id,
    )
    .await
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to sign attestation: {e}"),
        )
    })?;

    Ok(Json(CompetitionAttestationResponse {
        oracle: attestation.oracle,
        data: attestation.data,
        placements: req.placements,
    }))
}

// ---------- helpers ----------

async fn fetch_lichess_game(
    state: &AppState,
    game_id: &str,
) -> Result<LichessGameExport, (StatusCode, String)> {
    let url = format!("{LICHESS_GAME_EXPORT_URL}/{game_id}");

    let response = state
        .http
        .get(&url)
        .header(ACCEPT, "application/json")
        .send()
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("lichess request failed: {e}")))?;

    let status = response.status();
    if status == reqwest::StatusCode::NOT_FOUND {
        return Err((StatusCode::NOT_FOUND, format!("game `{game_id}` not found")));
    }
    if !status.is_success() {
        return Err((
            StatusCode::BAD_GATEWAY,
            format!("lichess returned HTTP {status}"),
        ));
    }

    response
        .json::<LichessGameExport>()
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("invalid JSON from lichess: {e}")))
}

fn validate_game_id(game_id: &str) -> Result<(), (StatusCode, String)> {
    if game_id.len() == 8 && game_id.chars().all(|c| c.is_ascii_alphanumeric()) {
        Ok(())
    } else {
        Err((
            StatusCode::BAD_REQUEST,
            "game_id must be exactly 8 alphanumeric characters".into(),
        ))
    }
}

fn validate_placements(placements: &[RankedPlacementInput]) -> Result<(), (StatusCode, String)> {
    if placements.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "placements must contain at least one entry".into(),
        ));
    }

    for (i, entry) in placements.iter().enumerate() {
        if entry.rank == 0 {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("placements[{i}].rank must be greater than zero"),
            ));
        }
        if entry.player.is_zero() {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("placements[{i}].player must not be the zero address"),
            ));
        }

        for (_j, other) in placements.iter().enumerate().skip(i + 1) {
            if entry.rank == other.rank {
                return Err((
                    StatusCode::BAD_REQUEST,
                    format!("duplicate rank {} in placements", entry.rank),
                ));
            }
            if entry.player == other.player {
                return Err((
                    StatusCode::BAD_REQUEST,
                    format!("duplicate player {} in placements", entry.player),
                ));
            }
        }
    }

    Ok(())
}

/// The signer's chain_id must match the chain where `LichessOracle` is deployed
/// (the contract hashes it into the message). `AppState::from_env` already calls
/// `signer.with_chain_id(Some(CHAIN_ID))`, so this should always succeed.
fn signer_chain_id(state: &AppState) -> Result<u64, (StatusCode, String)> {
    state.signer.chain_id().ok_or((
        StatusCode::INTERNAL_SERVER_ERROR,
        "signer has no chain_id configured".into(),
    ))
}
