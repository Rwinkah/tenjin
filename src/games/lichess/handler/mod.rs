use std::sync::Arc;

use alloy::primitives::{Address, B256, Bytes, U256};
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use reqwest::header::ACCEPT;
use serde::{Deserialize, Serialize};

use crate::games::lichess::attestation::{
    RankedPlacement, build_binary_attestation, build_competition_attestation,
    build_competition_join_permit, build_competition_leave_permit,
    build_competition_void_attestation, build_draw_attestation, build_void_attestation,
};
use crate::games::lichess::competition_settlement::{self, PlacementEntry, RosterEntry};
use crate::games::lichess::join::{self, JoinKind};
use crate::games::lichess::models::{LichessColor, LichessGameExport, LichessStatus};
use crate::games::lichess::settlement::{
    CompetitionSettlementOption, check_competition_void_eligible, check_void_eligible,
    prepare_competition_settle,
};
use crate::shared::min_stake::{self, GameKind};
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

// ---------- Draw (1v1) attestation ----------

#[derive(Debug, Deserialize)]
pub struct DrawAttestationRequest {
    pub wager_id: B256,
    pub game_id: String,
}

#[derive(Debug, Serialize)]
pub struct DrawAttestationResponse {
    pub oracle: Address,
    pub data: Bytes,
    pub game_id: String,
    pub raw_status: LichessStatus,
}

/// `POST /attestations/draw`
///
/// Fetches the Lichess game, requires a drawn result, then returns a signed attestation for `lcSettleDraw`.
pub async fn draw_attestation(
    State(state): State<Arc<AppState>>,
    Json(req): Json<DrawAttestationRequest>,
) -> Result<Json<DrawAttestationResponse>, (StatusCode, String)> {
    validate_game_id(&req.game_id)?;

    let game = fetch_lichess_game(&state, &req.game_id).await?;

    let is_draw = matches!(game.status, LichessStatus::Draw | LichessStatus::Stalemate);
    if !is_draw {
        return Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            format!(
                "game `{}` is not a draw: status `{:?}`, winner `{:?}`",
                game.id, game.status, game.winner
            ),
        ));
    }

    let chain_id = signer_chain_id(&state)?;

    let attestation =
        build_draw_attestation(&state.signer, state.oracle_address, req.wager_id, chain_id)
            .await
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("failed to sign attestation: {e}"),
                )
            })?;

    Ok(Json(DrawAttestationResponse {
        oracle: attestation.oracle,
        data: attestation.data,
        game_id: game.id,
        raw_status: game.status,
    }))
}

// ---------- Void (1v1) attestation ----------

#[derive(Debug, Deserialize)]
pub struct VoidAttestationRequest {
    pub wager_id: B256,
    pub game_id: String,
}

#[derive(Debug, Serialize)]
pub struct VoidAttestationResponse {
    pub oracle: Address,
    pub data: Bytes,
    pub game_id: String,
    pub raw_status: LichessStatus,
}

/// `POST /attestations/void`
///
/// Fetches the Lichess game, requires a void result, then returns a signed attestation for `lcSettleVoid`.
pub async fn void_attestation(
    State(state): State<Arc<AppState>>,
    Json(req): Json<VoidAttestationRequest>,
) -> Result<Json<VoidAttestationResponse>, (StatusCode, String)> {
    validate_game_id(&req.game_id)?;

    let game = fetch_lichess_game(&state, &req.game_id).await?;

    check_void_eligible(&game)?;

    let chain_id = signer_chain_id(&state)?;

    let attestation =
        build_void_attestation(&state.signer, state.oracle_address, req.wager_id, chain_id)
            .await
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("failed to sign attestation: {e}"),
                )
            })?;

    Ok(Json(VoidAttestationResponse {
        oracle: attestation.oracle,
        data: attestation.data,
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

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct RosterEntryInput {
    pub wallet: Address,
    pub lichess_username: String,
}

#[derive(Debug, Deserialize)]
pub struct CompetitionAttestationRequest {
    pub wager_id: B256,
    pub lichess_event_id: Option<String>,
    /// Ranked placements — order must match the array passed to `lcSettleCompetition`.
    pub placements: Vec<RankedPlacementInput>,
    pub roster: Option<Vec<RosterEntryInput>>,
}

#[derive(Debug, Serialize)]
pub struct CompetitionAttestationResponse {
    pub oracle: Address,
    pub data: Bytes,
    pub placements: Vec<RankedPlacementInput>,
}

/// `POST /attestations/competition`
///
/// Signs a ranked placement attestation verified against Lichess arena results.
pub async fn competition_attestation(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CompetitionAttestationRequest>,
) -> Result<Json<CompetitionAttestationResponse>, (StatusCode, String)> {
    validate_placements(&req.placements)?;

    let lichess_event_id = req
        .lichess_event_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                "lichess_event_id is required for verified competition settlement".into(),
            )
        })?;

    let roster_inputs = req.roster.as_deref().ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            "roster is required for verified competition settlement".into(),
        )
    })?;

    if roster_inputs.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "roster must not be empty".into()));
    }

    let placement_entries: Vec<PlacementEntry> = req
        .placements
        .iter()
        .map(|entry| PlacementEntry {
            rank: entry.rank,
            player: entry.player,
        })
        .collect();

    let roster_entries: Vec<RosterEntry> = roster_inputs
        .iter()
        .map(|entry| RosterEntry {
            wallet: entry.wallet,
            lichess_username: entry.lichess_username.clone(),
        })
        .collect();

    competition_settlement::validate_competition_placements(
        &state,
        lichess_event_id,
        &placement_entries,
        &roster_entries,
    )
    .await
    .map_err(|reason| (StatusCode::FORBIDDEN, reason))?;

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

// ---------- Competition prepare-settle ----------

#[derive(Debug, Deserialize)]
pub struct CompetitionPrepareSettleRequest {
    pub participants_count: u64,
    pub min_joiners_to_settle: u64,
}

#[derive(Debug, Serialize)]
pub struct CompetitionPrepareSettleResponse {
    pub options: Vec<String>,
    pub recommended: Option<String>,
    pub ranked_eligible: bool,
    pub void_eligible: bool,
    pub reason: Option<String>,
}

fn settlement_option_label(option: CompetitionSettlementOption) -> &'static str {
    match option {
        CompetitionSettlementOption::Ranked => "ranked",
        CompetitionSettlementOption::Void => "void",
    }
}

/// `POST /attestations/competition/prepare-settle`
///
/// Returns which settlement paths Tenjin will attest for the given on-chain join counts.
pub async fn competition_prepare_settle(
    Json(req): Json<CompetitionPrepareSettleRequest>,
) -> Result<Json<CompetitionPrepareSettleResponse>, (StatusCode, String)> {
    if req.min_joiners_to_settle == 0 {
        return Err((
            StatusCode::BAD_REQUEST,
            "min_joiners_to_settle must be greater than zero".into(),
        ));
    }

    let prep = prepare_competition_settle(req.participants_count, req.min_joiners_to_settle);
    let mut options = Vec::new();
    if prep.void_eligible {
        options.push(settlement_option_label(CompetitionSettlementOption::Void).into());
    }
    if prep.ranked_eligible {
        options.push(settlement_option_label(CompetitionSettlementOption::Ranked).into());
    }

    Ok(Json(CompetitionPrepareSettleResponse {
        options,
        recommended: prep
            .recommended
            .map(|option| settlement_option_label(option).into()),
        ranked_eligible: prep.ranked_eligible,
        void_eligible: prep.void_eligible,
        reason: prep.reason,
    }))
}

// ---------- Competition void attestation ----------

#[derive(Debug, Deserialize)]
pub struct CompetitionVoidAttestationRequest {
    pub wager_id: B256,
    pub participants_count: u64,
    pub min_joiners_to_settle: u64,
}

#[derive(Debug, Serialize)]
pub struct CompetitionVoidAttestationResponse {
    pub oracle: Address,
    pub data: Bytes,
    pub settlement: String,
    pub reason: Option<String>,
}

/// `POST /attestations/competition-void`
///
/// Signs a multi-party void attestation for `lcVoidCompetition` when joiners are below the minimum.
pub async fn competition_void_attestation(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CompetitionVoidAttestationRequest>,
) -> Result<Json<CompetitionVoidAttestationResponse>, (StatusCode, String)> {
    check_competition_void_eligible(req.participants_count, req.min_joiners_to_settle)?;

    let prep = prepare_competition_settle(req.participants_count, req.min_joiners_to_settle);
    let chain_id = signer_chain_id(&state)?;

    let attestation = build_competition_void_attestation(
        &state.signer,
        state.oracle_address,
        req.wager_id,
        chain_id,
    )
    .await
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to sign attestation: {e}"),
        )
    })?;

    Ok(Json(CompetitionVoidAttestationResponse {
        oracle: attestation.oracle,
        data: attestation.data,
        settlement: settlement_option_label(CompetitionSettlementOption::Void).into(),
        reason: prep.reason,
    }))
}

// ---------- Unified join permit ----------

#[derive(Debug, Deserialize)]
pub struct JoinAttestationRequest {
    pub kind: String,
    pub wager_id: B256,
    pub joiner: Address,
    /// Exact `gameId` string from `WagerCreated` (required for `binary`).
    #[serde(default)]
    pub game_id: Option<String>,
    #[serde(default)]
    pub joiner_lichess_username: Option<String>,
    #[serde(default)]
    pub challenge_id: Option<String>,
    /// Campaign slug. Missing → `platform`.
    #[serde(default)]
    pub campaign: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct JoinAttestationResponse {
    pub oracle: Address,
    pub deadline: U256,
    /// Raw EIP-712 signature for `lcJoinWithPermit` (not `OracleAttestation.data`).
    pub signature: String,
}

async fn sign_join_permit(
    state: &AppState,
    wager_id: B256,
    joiner: Address,
) -> Result<JoinAttestationResponse, (StatusCode, String)> {
    let chain_id = signer_chain_id(state)?;

    let permit = build_competition_join_permit(
        &state.signer,
        state.oracle_address,
        wager_id,
        joiner,
        chain_id,
    )
    .await
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to sign join permit: {e}"),
        )
    })?;

    Ok(JoinAttestationResponse {
        oracle: permit.oracle,
        deadline: permit.deadline,
        signature: permit.signature.to_string(),
    })
}

async fn issue_join_permit(
    state: &AppState,
    kind: JoinKind,
    wager_id: B256,
    joiner: Address,
    game_id: Option<&str>,
    joiner_lichess_username: Option<&str>,
    challenge_id: Option<&str>,
    campaign: Option<&str>,
) -> Result<JoinAttestationResponse, (StatusCode, String)> {
    if joiner.is_zero() {
        return Err((
            StatusCode::BAD_REQUEST,
            "joiner must not be the zero address".into(),
        ));
    }

    min_stake::enforce_min_stake(state, campaign, GameKind::Lichess, wager_id).await?;

    match kind {
        JoinKind::Binary => {
            let game_id = game_id.unwrap_or("").trim();
            let joiner_username = joiner_lichess_username.unwrap_or("").trim();
            join::validate_binary_join(state, wager_id, game_id, joiner_username, challenge_id)
                .await
                .map_err(|reason| (StatusCode::FORBIDDEN, reason))?;
        }
        JoinKind::Competition => {
            join::validate_competition_join(state, wager_id)
                .await
                .map_err(|reason| (StatusCode::FORBIDDEN, reason))?;
        }
    }

    sign_join_permit(state, wager_id, joiner).await
}

/// `POST /attestations/join`
///
/// Validates join eligibility by `kind`, then signs `CompetitionJoin` for `lcJoinWithPermit`.
pub async fn join_attestation(
    State(state): State<Arc<AppState>>,
    Json(req): Json<JoinAttestationRequest>,
) -> Result<Json<JoinAttestationResponse>, (StatusCode, String)> {
    let kind = JoinKind::parse(&req.kind).map_err(|reason| (StatusCode::BAD_REQUEST, reason))?;

    let response = issue_join_permit(
        &state,
        kind,
        req.wager_id,
        req.joiner,
        req.game_id.as_deref(),
        req.joiner_lichess_username.as_deref(),
        req.challenge_id.as_deref(),
        req.campaign.as_deref(),
    )
    .await?;

    Ok(Json(response))
}

// ---------- Competition join permit (legacy) ----------

#[derive(Debug, Deserialize)]
pub struct CompetitionJoinAttestationRequest {
    pub wager_id: B256,
    pub joiner: Address,
    #[serde(default)]
    pub campaign: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct CompetitionJoinAttestationResponse {
    pub oracle: Address,
    pub deadline: U256,
    /// Raw EIP-712 signature for `lcJoinWithPermit` (not `OracleAttestation.data`).
    pub signature: String,
}

/// `POST /attestations/competition-join`
///
/// Deprecated — use `POST /attestations/join` with `"kind": "competition"`.
pub async fn competition_join_attestation(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CompetitionJoinAttestationRequest>,
) -> Result<Json<CompetitionJoinAttestationResponse>, (StatusCode, String)> {
    let unified = issue_join_permit(
        &state,
        JoinKind::Competition,
        req.wager_id,
        req.joiner,
        None,
        None,
        None,
        req.campaign.as_deref(),
    )
    .await?;

    Ok(Json(CompetitionJoinAttestationResponse {
        oracle: unified.oracle,
        deadline: unified.deadline,
        signature: unified.signature,
    }))
}

// ---------- Competition leave permit ----------

#[derive(Debug, Deserialize)]
pub struct LeaveAttestationRequest {
    pub wager_id: B256,
    pub leaver: Address,
}

async fn sign_leave_permit(
    state: &AppState,
    wager_id: B256,
    leaver: Address,
) -> Result<JoinAttestationResponse, (StatusCode, String)> {
    let chain_id = signer_chain_id(state)?;

    let permit = build_competition_leave_permit(
        &state.signer,
        state.oracle_address,
        wager_id,
        leaver,
        chain_id,
    )
    .await
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to sign leave permit: {e}"),
        )
    })?;

    Ok(JoinAttestationResponse {
        oracle: permit.oracle,
        deadline: permit.deadline,
        signature: permit.signature.to_string(),
    })
}

/// `POST /attestations/leave`
///
/// Validates on-chain leave eligibility, then signs `CompetitionLeave` for `lcLeaveCompetitionWithPermit`.
pub async fn leave_attestation(
    State(state): State<Arc<AppState>>,
    Json(req): Json<LeaveAttestationRequest>,
) -> Result<Json<JoinAttestationResponse>, (StatusCode, String)> {
    if req.leaver.is_zero() {
        return Err((
            StatusCode::BAD_REQUEST,
            "leaver must not be the zero address".into(),
        ));
    }

    join::validate_competition_leave(&state, req.wager_id, req.leaver)
        .await
        .map_err(|reason| (StatusCode::FORBIDDEN, reason))?;

    sign_leave_permit(&state, req.wager_id, req.leaver)
        .await
        .map(Json)
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
        .map_err(|e| {
            (
                StatusCode::BAD_GATEWAY,
                format!("lichess request failed: {e}"),
            )
        })?;

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

    response.json::<LichessGameExport>().await.map_err(|e| {
        (
            StatusCode::BAD_GATEWAY,
            format!("invalid JSON from lichess: {e}"),
        )
    })
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
