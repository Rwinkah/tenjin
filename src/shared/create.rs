//! Create permits for `*CreateMatchWithPermit` / `*CreateCompetitionWithPermit`.
//!
//! wager-backend calls this only after the creator passes a passkey check. The permit binds the
//! creator, `keccak256(gameId)` and the exact wei sent, so it opens one specific wager
//! (game ids are single-use on-chain). Service-key only (see `shared::service_auth`).

use std::str::FromStr;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use alloy::primitives::{Address, B256, U256, keccak256};
use alloy::signers::Signer;
use alloy::sol;
use alloy::sol_types::{Eip712Domain, SolStruct};
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};

use crate::shared::models::AppState;

/// The browser sends the create tx right after the permit; keep the window short.
pub const CREATE_PERMIT_TTL_SECS: u64 = 600;

const MAX_GAME_ID_LEN: usize = 4096;

sol! {
    #[derive(Debug)]
    struct WagerCreate {
        address creator;
        bytes32 gameIdHash;
        uint256 value;
        uint256 deadline;
    }
}

#[derive(Debug, Clone, Copy)]
pub enum CreateGame {
    Lichess,
    Football,
    Strava,
    Fpl,
}

impl CreateGame {
    fn domain_name(self) -> &'static str {
        match self {
            CreateGame::Lichess => "LichessOracle",
            CreateGame::Football => "FootballOracle",
            CreateGame::Strava => "StravaOracle",
            CreateGame::Fpl => "FplOracle",
        }
    }

    fn oracle(self, state: &AppState) -> Address {
        match self {
            CreateGame::Lichess => state.oracle_address,
            CreateGame::Football => state.football_oracle_address,
            CreateGame::Strava => state.strava_oracle_address,
            CreateGame::Fpl => state.fpl_oracle_address,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct CreatePermitRequest {
    pub creator: Address,
    pub game_id: String,
    /// Exact `msg.value` in wei (decimal or `0x` hex string).
    pub value: String,
}

#[derive(Debug, Serialize)]
pub struct CreatePermitResponse {
    pub oracle: Address,
    pub deadline: u64,
    pub signature: String,
}

fn bad_request(msg: &str) -> (StatusCode, String) {
    (StatusCode::BAD_REQUEST, msg.to_string())
}

/// Checks the request and returns `(gameIdHash, value)`.
fn validate(req: &CreatePermitRequest) -> Result<(B256, U256), (StatusCode, String)> {
    if req.creator.is_zero() {
        return Err(bad_request("creator must not be the zero address"));
    }
    let game_id = req.game_id.as_str();
    if game_id.trim().is_empty() {
        return Err(bad_request("game_id is required"));
    }
    if game_id.len() > MAX_GAME_ID_LEN {
        return Err(bad_request("game_id is too long"));
    }
    let value = U256::from_str(req.value.trim()).map_err(|_| bad_request("value must be a wei amount"))?;
    if value.is_zero() {
        return Err(bad_request("value must be greater than zero"));
    }
    // Matches `wgGameIdKey`: keccak256(abi.encodePacked(gameId)).
    Ok((keccak256(game_id.as_bytes()), value))
}

async fn issue(
    state: &AppState,
    game: CreateGame,
    req: CreatePermitRequest,
) -> Result<CreatePermitResponse, (StatusCode, String)> {
    let (game_id_hash, value) = validate(&req)?;

    let oracle = game.oracle(state);
    if oracle.is_zero() {
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("{} address is not configured", game.domain_name()),
        ));
    }
    let chain_id = state.signer.chain_id().ok_or((
        StatusCode::INTERNAL_SERVER_ERROR,
        "signer has no chain_id configured".to_string(),
    ))?;
    let deadline = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
        .saturating_add(CREATE_PERMIT_TTL_SECS);

    let domain = Eip712Domain::new(
        Some(game.domain_name().into()),
        Some("1".into()),
        Some(U256::from(chain_id)),
        Some(oracle),
        None,
    );
    let message = WagerCreate {
        creator: req.creator,
        gameIdHash: game_id_hash,
        value,
        deadline: U256::from(deadline),
    };
    let signature = state
        .signer
        .sign_hash(&message.eip712_signing_hash(&domain))
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("failed to sign create permit: {e}")))?;

    Ok(CreatePermitResponse {
        oracle,
        deadline,
        signature: signature.to_string(),
    })
}

/// `POST /attestations/create` (Lichess).
pub async fn lichess_create_attestation(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreatePermitRequest>,
) -> Result<Json<CreatePermitResponse>, (StatusCode, String)> {
    issue(&state, CreateGame::Lichess, req).await.map(Json)
}

/// `POST /football/attestations/create`.
pub async fn football_create_attestation(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreatePermitRequest>,
) -> Result<Json<CreatePermitResponse>, (StatusCode, String)> {
    issue(&state, CreateGame::Football, req).await.map(Json)
}

/// `POST /strava/attestations/create`.
pub async fn strava_create_attestation(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreatePermitRequest>,
) -> Result<Json<CreatePermitResponse>, (StatusCode, String)> {
    issue(&state, CreateGame::Strava, req).await.map(Json)
}

/// `POST /fpl/attestations/create`.
pub async fn fpl_create_attestation(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreatePermitRequest>,
) -> Result<Json<CreatePermitResponse>, (StatusCode, String)> {
    issue(&state, CreateGame::Fpl, req).await.map(Json)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::signers::local::PrivateKeySigner;

    fn req(creator: Address, game_id: &str, value: &str) -> CreatePermitRequest {
        CreatePermitRequest {
            creator,
            game_id: game_id.to_string(),
            value: value.to_string(),
        }
    }

    #[test]
    fn type_string_matches_oracle_typehash() {
        // Must equal WAGER_CREATE_TYPEHASH's preimage in every *Oracle.sol.
        assert_eq!(
            WagerCreate::eip712_encode_type(),
            "WagerCreate(address creator,bytes32 gameIdHash,uint256 value,uint256 deadline)"
        );
    }

    #[test]
    fn validates_inputs() {
        let creator = Address::repeat_byte(1);
        assert!(validate(&req(Address::ZERO, "g", "1")).is_err());
        assert!(validate(&req(creator, " ", "1")).is_err());
        assert!(validate(&req(creator, "g", "0")).is_err());
        assert!(validate(&req(creator, "g", "abc")).is_err());
        let (hash, value) = validate(&req(creator, "g", "0x10")).unwrap();
        assert_eq!(value, U256::from(16));
        assert_eq!(hash, keccak256(b"g"));
    }

    #[tokio::test]
    async fn signature_recovers_to_signer_under_game_domain() {
        let signer = PrivateKeySigner::random().with_chain_id(Some(31337));
        let oracle = Address::repeat_byte(9);
        let domain = Eip712Domain::new(
            Some("FplOracle".into()),
            Some("1".into()),
            Some(U256::from(31337u64)),
            Some(oracle),
            None,
        );
        let message = WagerCreate {
            creator: Address::repeat_byte(1),
            gameIdHash: keccak256(b"fpl-game"),
            value: U256::from(10u64),
            deadline: U256::from(100u64),
        };
        let hash = message.eip712_signing_hash(&domain);
        let signature = signer.sign_hash(&hash).await.unwrap();
        assert_eq!(signature.recover_address_from_prehash(&hash).unwrap(), signer.address());
    }
}
