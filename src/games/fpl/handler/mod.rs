use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use alloy::primitives::{Address, Bytes, B256, U256};
use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::games::fpl::api::{
    entry_in_classic_league, entry_in_h2h_league, event_is_data_checked, event_is_finished,
    fetch_bootstrap, fetch_classic_standings, fetch_entry, fetch_gw_points, fetch_h2h_match,
};
use crate::games::fpl::attestation::{
    build_binary_attestation, build_competition_attestation, build_competition_void_attestation,
    build_draw_attestation, build_join_permit, build_void_attestation, RankedPlacement,
};
use crate::games::fpl::models::{FplGameId, FplMarketKind};
use crate::games::fpl::settlement::{
    classify_gameweek, classify_h2h, classify_league, competition_placements, BinaryClass, H2hScore,
    LeagueStanding, RosterStanding, SideScore,
};
use crate::shared::models::AppState;

const ATTESTATION_TTL_SECS: u64 = 7 * 24 * 3600;

pub async fn health_handler(State(app_state): State<Arc<AppState>>) -> String {
    format!(
        "fpl OK (signer: {}, oracle: {})",
        app_state.signer.address(),
        app_state.fpl_oracle_address
    )
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn now_deadline() -> u64 {
    now_unix().saturating_add(ATTESTATION_TTL_SECS)
}

fn signer_chain_id(state: &AppState) -> Result<u64, (StatusCode, String)> {
    state.signer.chain_id().ok_or((
        StatusCode::INTERNAL_SERVER_ERROR,
        "signer has no chain_id configured".into(),
    ))
}

fn fpl_oracle(state: &AppState) -> Result<Address, (StatusCode, String)> {
    if state.fpl_oracle_address == Address::ZERO {
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            "FPL_ORACLE_ADDRESS is not configured".into(),
        ));
    }
    Ok(state.fpl_oracle_address)
}

fn ensure_join_open(finished: Option<bool>) -> Result<(), (StatusCode, String)> {
    match finished {
        None => Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            "could not load FPL gameweek".into(),
        )),
        Some(true) => Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            "FPL gameweek is finished".into(),
        )),
        Some(false) => Ok(()),
    }
}

fn sign_error(ctx: &str, err: alloy::signers::Error) -> (StatusCode, String) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        format!("failed to sign {ctx}: {err}"),
    )
}

#[derive(Debug, Deserialize)]
pub struct JoinPermitRequest {
    pub wager_id: B256,
    pub joiner: Address,
    pub game_id: String,
    #[serde(default)]
    pub joiner_display_name: Option<String>,
    #[serde(default)]
    pub joiner_entry_id: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct JoinPermitResponse {
    pub oracle: Address,
    pub deadline: u64,
    pub signature: Bytes,
}

pub async fn join_attestation(
    State(state): State<Arc<AppState>>,
    Json(req): Json<JoinPermitRequest>,
) -> Result<Json<JoinPermitResponse>, (StatusCode, String)> {
    let meta = FplGameId::parse(&req.game_id).map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    meta.ensure_directed_joiner(req.joiner_display_name.as_deref())
        .map_err(|e| (StatusCode::FORBIDDEN, e))?;

    let bootstrap = fetch_bootstrap(&state).await?;
    let join_event = meta
        .join_event_id()
        .map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    ensure_join_open(event_is_finished(&bootstrap, join_event))?;

    if meta.requires_league() {
        let league_id = meta
            .league_id_required()
            .map_err(|e| (StatusCode::BAD_REQUEST, e))?;
        if let Some(entry_id) = req.joiner_entry_id {
            let entry = fetch_entry(&state, entry_id).await?;
            match meta.kind {
                FplMarketKind::H2h => {
                    if !entry_in_h2h_league(&entry, league_id) {
                        return Err((
                            StatusCode::FORBIDDEN,
                            "joiner is not in that FPL H2H league".into(),
                        ));
                    }
                    let match_row = fetch_h2h_match(
                        &state,
                        league_id,
                        join_event,
                        meta.creator_entry_id,
                        entry_id,
                    )
                    .await?;
                    if match_row.is_none() {
                        return Err((
                            StatusCode::FORBIDDEN,
                            "joiner is not the FPL H2H opponent for that gameweek".into(),
                        ));
                    }
                }
                FplMarketKind::League | FplMarketKind::LeaguePot => {
                    if !entry_in_classic_league(&entry, league_id) {
                        return Err((
                            StatusCode::FORBIDDEN,
                            "joiner is not in that FPL classic league".into(),
                        ));
                    }
                }
                FplMarketKind::Gameweek => {}
            }
        }
    }

    let chain_id = signer_chain_id(&state)?;
    let oracle = fpl_oracle(&state)?;
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
    .map_err(|e| sign_error("join permit", e))?;

    Ok(Json(JoinPermitResponse {
        oracle: permit.oracle,
        deadline: permit.deadline,
        signature: permit.signature,
    }))
}

#[derive(Debug, Deserialize)]
pub struct SettleAttestationRequest {
    pub wager_id: B256,
    pub game_id: String,
    pub creator_address: Address,
    pub joiner_address: Address,
    #[serde(default)]
    pub joiner_entry_id: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct BinaryAttestationResponse {
    pub oracle: Address,
    pub data: Bytes,
    pub winner: Address,
    pub loser: Address,
    pub event: u32,
    pub class: String,
}

#[derive(Debug, Serialize)]
pub struct SimpleAttestationResponse {
    pub oracle: Address,
    pub data: Bytes,
    pub event: u32,
    pub class: String,
}

async fn classify_binary(
    state: &AppState,
    meta: &FplGameId,
    joiner_entry_override: Option<u64>,
) -> Result<(u32, BinaryClass), (StatusCode, String)> {
    let bootstrap = fetch_bootstrap(state).await?;
    let event = meta
        .settle_event_id()
        .map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    let data_checked = event_is_data_checked(&bootstrap, event);
    let joiner_entry = joiner_entry_override.or(meta.joiner_entry_id).unwrap_or(0);
    if joiner_entry == 0 && meta.kind != FplMarketKind::LeaguePot {
        return Err((
            StatusCode::BAD_REQUEST,
            "gameId missing joinerEntryId".into(),
        ));
    }

    let class = match meta.kind {
        FplMarketKind::Gameweek => {
            let creator_entry = fetch_entry(state, meta.creator_entry_id).await.ok();
            let joiner_entry_payload = fetch_entry(state, joiner_entry).await.ok();
            let creator = SideScore {
                points: fetch_gw_points(state, meta.creator_entry_id, event).await?,
                started_event: creator_entry.and_then(|e| e.started_event),
            };
            let joiner = SideScore {
                points: fetch_gw_points(state, joiner_entry, event).await?,
                started_event: joiner_entry_payload.and_then(|e| e.started_event),
            };
            classify_gameweek(event, data_checked, creator, joiner)
        }
        FplMarketKind::League => {
            let league_id = meta
                .league_id_required()
                .map_err(|e| (StatusCode::BAD_REQUEST, e))?;
            let standings =
                fetch_classic_standings(state, league_id, &[meta.creator_entry_id, joiner_entry])
                    .await?;
            let creator_entry = fetch_entry(state, meta.creator_entry_id).await.ok();
            let joiner_entry_payload = fetch_entry(state, joiner_entry).await.ok();
            let creator_row = standings.get(&meta.creator_entry_id);
            let joiner_row = standings.get(&joiner_entry);
            classify_league(
                data_checked,
                event,
                LeagueStanding {
                    rank: creator_row.and_then(|r| r.rank),
                    total: creator_row.and_then(|r| r.total),
                },
                LeagueStanding {
                    rank: joiner_row.and_then(|r| r.rank),
                    total: joiner_row.and_then(|r| r.total),
                },
                creator_entry.and_then(|e| e.started_event),
                joiner_entry_payload.and_then(|e| e.started_event),
            )
        }
        FplMarketKind::H2h => {
            let league_id = meta
                .league_id_required()
                .map_err(|e| (StatusCode::BAD_REQUEST, e))?;
            let match_row =
                fetch_h2h_match(state, league_id, event, meta.creator_entry_id, joiner_entry)
                    .await?;
            let creator_entry = fetch_entry(state, meta.creator_entry_id).await.ok();
            let joiner_entry_payload = fetch_entry(state, joiner_entry).await.ok();
            let score = match match_row {
                Some(row) => {
                    let creator_is_one = row.entry_1_entry == Some(meta.creator_entry_id);
                    H2hScore {
                        found: true,
                        creator_points: if creator_is_one {
                            row.entry_1_points
                        } else {
                            row.entry_2_points
                        },
                        joiner_points: if creator_is_one {
                            row.entry_2_points
                        } else {
                            row.entry_1_points
                        },
                    }
                }
                None => H2hScore {
                    found: false,
                    creator_points: None,
                    joiner_points: None,
                },
            };
            classify_h2h(
                event,
                data_checked,
                score,
                creator_entry.and_then(|e| e.started_event),
                joiner_entry_payload.and_then(|e| e.started_event),
            )
        }
        FplMarketKind::LeaguePot => {
            return Err((
                StatusCode::BAD_REQUEST,
                "league pots settle via /attestations/competition".into(),
            ));
        }
    };
    Ok((event, class))
}

fn class_label(class: BinaryClass) -> &'static str {
    match class {
        BinaryClass::CreatorWin => "creator_win",
        BinaryClass::JoinerWin => "joiner_win",
        BinaryClass::Draw => "draw",
        BinaryClass::Void => "void",
        BinaryClass::NotReady => "not_ready",
    }
}

fn reject_not_ready(event: u32, class: BinaryClass) -> Result<(), (StatusCode, String)> {
    if class == BinaryClass::NotReady {
        return Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            format!(
                "gameweek {event} is not fully checked yet — settle uses official FPL points after the GW is fully checked (bonus included)"
            ),
        ));
    }
    Ok(())
}

pub async fn binary_attestation(
    State(state): State<Arc<AppState>>,
    Json(req): Json<SettleAttestationRequest>,
) -> Result<Json<BinaryAttestationResponse>, (StatusCode, String)> {
    let meta = FplGameId::parse(&req.game_id).map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    let (event, class) = classify_binary(&state, &meta, req.joiner_entry_id).await?;
    reject_not_ready(event, class)?;
    let (winner, loser) = match class {
        BinaryClass::CreatorWin => (req.creator_address, req.joiner_address),
        BinaryClass::JoinerWin => (req.joiner_address, req.creator_address),
        BinaryClass::Draw => {
            return Err((
                StatusCode::UNPROCESSABLE_ENTITY,
                "result is a draw — use /attestations/draw".into(),
            ));
        }
        BinaryClass::Void => {
            return Err((
                StatusCode::UNPROCESSABLE_ENTITY,
                "result is void — use /attestations/void".into(),
            ));
        }
        BinaryClass::NotReady => unreachable!(),
    };

    let chain_id = signer_chain_id(&state)?;
    let oracle = fpl_oracle(&state)?;
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
    .map_err(|e| sign_error("binary attestation", e))?;

    Ok(Json(BinaryAttestationResponse {
        oracle: attestation.oracle,
        data: attestation.data,
        winner,
        loser,
        event,
        class: class_label(class).into(),
    }))
}

pub async fn draw_attestation(
    State(state): State<Arc<AppState>>,
    Json(req): Json<SettleAttestationRequest>,
) -> Result<Json<SimpleAttestationResponse>, (StatusCode, String)> {
    let meta = FplGameId::parse(&req.game_id).map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    let (event, class) = classify_binary(&state, &meta, req.joiner_entry_id).await?;
    reject_not_ready(event, class)?;
    if class != BinaryClass::Draw {
        return Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            format!("result is `{}`, not a draw", class_label(class)),
        ));
    }
    let chain_id = signer_chain_id(&state)?;
    let oracle = fpl_oracle(&state)?;
    let deadline = now_deadline();
    let attestation =
        build_draw_attestation(&state.signer, oracle, req.wager_id, chain_id, deadline)
            .await
            .map_err(|e| sign_error("draw attestation", e))?;
    Ok(Json(SimpleAttestationResponse {
        oracle: attestation.oracle,
        data: attestation.data,
        event,
        class: class_label(class).into(),
    }))
}

pub async fn void_attestation(
    State(state): State<Arc<AppState>>,
    Json(req): Json<SettleAttestationRequest>,
) -> Result<Json<SimpleAttestationResponse>, (StatusCode, String)> {
    let meta = FplGameId::parse(&req.game_id).map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    let (event, class) = classify_binary(&state, &meta, req.joiner_entry_id).await?;
    reject_not_ready(event, class)?;
    if class != BinaryClass::Void {
        return Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            format!("result is `{}`, not void", class_label(class)),
        ));
    }
    let chain_id = signer_chain_id(&state)?;
    let oracle = fpl_oracle(&state)?;
    let deadline = now_deadline();
    let attestation =
        build_void_attestation(&state.signer, oracle, req.wager_id, chain_id, deadline)
            .await
            .map_err(|e| sign_error("void attestation", e))?;
    Ok(Json(SimpleAttestationResponse {
        oracle: attestation.oracle,
        data: attestation.data,
        event,
        class: class_label(class).into(),
    }))
}

#[derive(Debug, Deserialize)]
pub struct RosterInput {
    pub wallet: Address,
    pub entry_id: u64,
}

#[derive(Debug, Deserialize)]
pub struct CompetitionAttestationRequest {
    pub wager_id: B256,
    pub game_id: String,
    pub roster: Vec<RosterInput>,
}

#[derive(Debug, Serialize)]
pub struct PlacementWire {
    pub rank: u64,
    pub player: Address,
}

#[derive(Debug, Serialize)]
pub struct CompetitionAttestationResponse {
    pub oracle: Address,
    pub data: Bytes,
    pub placements: Vec<PlacementWire>,
    pub event: u32,
}

pub async fn competition_attestation(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CompetitionAttestationRequest>,
) -> Result<Json<CompetitionAttestationResponse>, (StatusCode, String)> {
    let meta = FplGameId::parse(&req.game_id).map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    if meta.kind != FplMarketKind::LeaguePot {
        return Err((
            StatusCode::BAD_REQUEST,
            "competition settlement only applies to fpl_league_pot".into(),
        ));
    }
    if req.roster.len() < 3 {
        return Err((
            StatusCode::BAD_REQUEST,
            "roster must include at least 3 participants".into(),
        ));
    }
    let bootstrap = fetch_bootstrap(&state).await?;
    let event = meta
        .settle_event_id()
        .map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    if !event_is_data_checked(&bootstrap, event) {
        return Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            format!(
                "gameweek {event} is not fully checked yet — settle uses official FPL points after the GW is fully checked (bonus included)"
            ),
        ));
    }
    let league_id = meta
        .league_id_required()
        .map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    let entry_ids: Vec<u64> = req.roster.iter().map(|row| row.entry_id).collect();
    let standings = fetch_classic_standings(&state, league_id, &entry_ids).await?;
    let roster: Vec<RosterStanding> = req
        .roster
        .iter()
        .map(|row| {
            let standing = standings.get(&row.entry_id);
            RosterStanding {
                wallet: row.wallet,
                entry_id: row.entry_id,
                rank: standing.and_then(|s| s.rank),
                total: standing.and_then(|s| s.total),
            }
        })
        .collect();
    let computed = competition_placements(&roster);
    if computed.is_empty() {
        return Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            "no scored league members in roster — use /attestations/competition-void".into(),
        ));
    }
    let winner_only: Vec<RankedPlacement> = computed
        .iter()
        .filter(|(rank, _)| *rank == 1)
        .map(|(_, player)| RankedPlacement {
            rank: U256::from(1u64),
            player: *player,
        })
        .collect();
    if winner_only.is_empty() {
        return Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            "could not determine league pot winner".into(),
        ));
    }

    let chain_id = signer_chain_id(&state)?;
    let oracle = fpl_oracle(&state)?;
    let deadline = now_deadline();
    let attestation = build_competition_attestation(
        &state.signer,
        oracle,
        req.wager_id,
        &winner_only,
        chain_id,
        deadline,
    )
    .await
    .map_err(|e| sign_error("competition attestation", e))?;

    Ok(Json(CompetitionAttestationResponse {
        oracle: attestation.oracle,
        data: attestation.data,
        placements: winner_only
            .iter()
            .map(|row| PlacementWire {
                rank: 1,
                player: row.player,
            })
            .collect(),
        event,
    }))
}

pub async fn competition_void_attestation(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CompetitionAttestationRequest>,
) -> Result<Json<SimpleAttestationResponse>, (StatusCode, String)> {
    let meta = FplGameId::parse(&req.game_id).map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    if meta.kind != FplMarketKind::LeaguePot {
        return Err((
            StatusCode::BAD_REQUEST,
            "competition void only applies to fpl_league_pot".into(),
        ));
    }
    let bootstrap = fetch_bootstrap(&state).await?;
    let event = meta
        .settle_event_id()
        .map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    if !event_is_data_checked(&bootstrap, event) {
        return Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            format!(
                "gameweek {event} is not fully checked yet — settle uses official FPL points after the GW is fully checked (bonus included)"
            ),
        ));
    }
    let league_id = meta
        .league_id_required()
        .map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    let entry_ids: Vec<u64> = req.roster.iter().map(|row| row.entry_id).collect();
    let standings = fetch_classic_standings(&state, league_id, &entry_ids).await?;
    let roster: Vec<RosterStanding> = req
        .roster
        .iter()
        .map(|row| {
            let standing = standings.get(&row.entry_id);
            RosterStanding {
                wallet: row.wallet,
                entry_id: row.entry_id,
                rank: standing.and_then(|s| s.rank),
                total: standing.and_then(|s| s.total),
            }
        })
        .collect();
    if !competition_placements(&roster).is_empty() {
        return Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            "scored standings exist — use /attestations/competition".into(),
        ));
    }

    let chain_id = signer_chain_id(&state)?;
    let oracle = fpl_oracle(&state)?;
    let deadline = now_deadline();
    let attestation = build_competition_void_attestation(
        &state.signer,
        oracle,
        req.wager_id,
        chain_id,
        deadline,
    )
    .await
    .map_err(|e| sign_error("competition void attestation", e))?;
    Ok(Json(SimpleAttestationResponse {
        oracle: attestation.oracle,
        data: attestation.data,
        event,
        class: "void".into(),
    }))
}

#[cfg(test)]
mod tests {
    use super::ensure_join_open;
    use axum::http::StatusCode;

    #[test]
    fn join_stays_open_until_gameweek_is_finished() {
        assert!(ensure_join_open(Some(false)).is_ok());
    }

    #[test]
    fn join_rejects_finished_gameweek() {
        let err = ensure_join_open(Some(true)).unwrap_err();
        assert_eq!(err.0, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(err.1.to_lowercase().contains("finished"));
    }

    #[test]
    fn join_rejects_unknown_gameweek() {
        let err = ensure_join_open(None).unwrap_err();
        assert_eq!(err.0, StatusCode::UNPROCESSABLE_ENTITY);
    }
}
