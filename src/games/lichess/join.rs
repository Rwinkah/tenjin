//! Join permit validation before Tenjin signs `CompetitionJoin`.

#[cfg(test)]
use std::time::{SystemTime, UNIX_EPOCH};

use alloy::consensus::BlockHeader;
use alloy::eips::BlockNumberOrTag;
use alloy::network::primitives::BlockResponse;
use alloy::primitives::{keccak256, Address, B256, U256};
use alloy::providers::{Provider, ProviderBuilder};
use alloy::sol;
use reqwest::header::{ACCEPT, AUTHORIZATION};
use serde::Deserialize;
use serde_json::Value;

use crate::shared::models::AppState;

const LICHESS_API_BASE: &str = "https://lichess.org";
const WG_FLAG_CREATED: u8 = 0;
const WG_FLAG_ACCEPTED: u8 = 1;
const WG_FLAG_CANCELLED: u8 = 2;
const WG_FLAG_EXPIRED: u8 = 3;
const WG_FLAG_COMPLETED: u8 = 4;
const WAGER_FORMAT_BINARY: u8 = 0;
const WAGER_FORMAT_MULTI_PARTY: u8 = 1;

sol! {
    #[sol(rpc)]
    contract LichessContractReader {
        function lcWagerJoinSnapshot(bytes32 wagerId)
            external
            view
            returns (
                bool exists,
                uint8 format,
                uint8 status,
                uint256 participantsCount,
                uint256 expiresAt,
                bytes32 gameIdKey
            );

        function lcWagerLeaveSnapshot(bytes32 wagerId, address leaver)
            external
            view
            returns (bool exists, bool canLeave);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JoinKind {
    Binary,
    Competition,
}

impl JoinKind {
    pub fn parse(raw: &str) -> Result<Self, String> {
        match raw.trim().to_lowercase().as_str() {
            "binary" | "1v1" | "match" => Ok(Self::Binary),
            "competition" | "tournament" | "arena" => Ok(Self::Competition),
            other => Err(format!("unsupported join kind `{other}`; use binary or competition")),
        }
    }
}

#[derive(Debug, Deserialize)]
struct BinaryGameIntent {
    v: u64,
    #[serde(default)]
    open: bool,
    opponent: Option<String>,
}

pub fn game_id_key(game_id: &str) -> B256 {
    keccak256(game_id.as_bytes())
}

pub fn parse_binary_intent(game_id: &str) -> Result<BinaryGameIntent, String> {
    let intent: BinaryGameIntent =
        serde_json::from_str(game_id).map_err(|e| format!("game_id is not valid JSON: {e}"))?;
    if intent.v != 1 {
        return Err(format!("unsupported game_id version {}", intent.v));
    }
    Ok(intent)
}

pub fn validate_binary_directed_opponent(
    intent: &BinaryGameIntent,
    joiner_lichess_username: &str,
) -> Result<(), String> {
    if intent.open {
        return Ok(());
    }
    let expected = intent
        .opponent
        .as_deref()
        .unwrap_or("")
        .trim()
        .to_lowercase();
    if expected.is_empty() {
        return Err("directed wager missing opponent in game_id".into());
    }
    let actual = joiner_lichess_username.trim().to_lowercase();
    if actual.is_empty() {
        return Err("joiner_lichess_username is required for binary join".into());
    }
    if actual != expected {
        return Err(format!(
            "joiner Lichess username `{actual}` does not match directed opponent `{expected}`"
        ));
    }
    Ok(())
}

#[derive(Debug)]
pub struct OnChainJoinSnapshot {
    pub exists: bool,
    pub format: u8,
    pub status: u8,
    pub participants_count: U256,
    pub expires_at: U256,
    /// Latest `block.timestamp` from the same RPC as the snapshot. The contract
    /// enforces `wgExpires > block.timestamp`; wall clock must not be mixed in.
    pub chain_now: U256,
    pub game_id_key: B256,
}

pub async fn fetch_on_chain_join_snapshot(
    state: &AppState,
    wager_id: B256,
) -> Result<OnChainJoinSnapshot, String> {
    let rpc_url = state
        .eth_rpc_url
        .as_deref()
        .ok_or_else(|| "ETH_RPC_URL is not configured".to_string())?;
    let contract = state
        .lichess_contract_address
        .ok_or_else(|| "LICHESS_CONTRACT_ADDRESS is not configured".to_string())?;

    let provider = ProviderBuilder::new().connect(rpc_url).await.map_err(|e| {
        format!("failed to connect RPC for join validation: {e}")
    })?;
    let chain_now = latest_chain_timestamp(&provider).await?;
    let reader = LichessContractReader::new(contract, provider);
    let snapshot = reader
        .lcWagerJoinSnapshot(wager_id)
        .call()
        .await
        .map_err(|e| format!("lcWagerJoinSnapshot call failed: {e}"))?;

    Ok(OnChainJoinSnapshot {
        exists: snapshot.exists,
        format: snapshot.format,
        status: snapshot.status,
        participants_count: snapshot.participantsCount,
        expires_at: snapshot.expiresAt,
        chain_now,
        game_id_key: snapshot.gameIdKey,
    })
}

async fn latest_chain_timestamp(provider: &impl Provider) -> Result<U256, String> {
    let block = provider
        .get_block_by_number(BlockNumberOrTag::Latest)
        .await
        .map_err(|e| format!("failed to read latest block: {e}"))?
        .ok_or_else(|| "latest block missing".to_string())?;
    Ok(U256::from(block.header().timestamp()))
}

#[cfg(test)]
fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before UNIX epoch")
        .as_secs()
}

fn validate_snapshot_base(snapshot: &OnChainJoinSnapshot, expected_format: u8) -> Result<(), String> {
    if !snapshot.exists {
        return Err("wager does not exist on-chain".into());
    }
    if snapshot.format != expected_format {
        return Err(format!(
            "wager format mismatch: expected {expected_format}, got {}",
            snapshot.format
        ));
    }
    Ok(())
}

fn validate_join_window_open(snapshot: &OnChainJoinSnapshot) -> Result<(), String> {
    if snapshot.expires_at <= snapshot.chain_now {
        return Err("wager join window has expired".into());
    }
    Ok(())
}

/// Binary (1v1): exactly one joiner; status must stay `CREATED` until that join.
pub fn validate_binary_joinable_snapshot(snapshot: &OnChainJoinSnapshot) -> Result<(), String> {
    validate_snapshot_base(snapshot, WAGER_FORMAT_BINARY)?;
    if snapshot.status != WG_FLAG_CREATED {
        return Err(format!(
            "wager is not open for joining (status {})",
            snapshot.status
        ));
    }
    if !snapshot.participants_count.is_zero() {
        return Err("wager already has a joiner".into());
    }
    validate_join_window_open(snapshot)
}

/// Multi-party competition: join while `CREATED` or `ACCEPTED` until expiry; capacity enforced on-chain.
pub fn validate_competition_joinable_snapshot(snapshot: &OnChainJoinSnapshot) -> Result<(), String> {
    validate_snapshot_base(snapshot, WAGER_FORMAT_MULTI_PARTY)?;
    match snapshot.status {
        WG_FLAG_CREATED | WG_FLAG_ACCEPTED => {}
        WG_FLAG_CANCELLED | WG_FLAG_EXPIRED | WG_FLAG_COMPLETED => {
            return Err(format!(
                "wager is not open for joining (status {})",
                snapshot.status
            ));
        }
        other => {
            return Err(format!(
                "wager is not open for joining (status {})",
                other
            ));
        }
    }
    validate_join_window_open(snapshot)
}

pub async fn validate_binary_join(
    state: &AppState,
    wager_id: B256,
    game_id: &str,
    joiner_lichess_username: &str,
    challenge_id: Option<&str>,
) -> Result<(), String> {
    if game_id.trim().is_empty() {
        return Err("game_id is required for binary join".into());
    }
    if joiner_lichess_username.trim().is_empty() {
        return Err("joiner_lichess_username is required for binary join".into());
    }

    let intent = parse_binary_intent(game_id)?;
    validate_binary_directed_opponent(&intent, joiner_lichess_username)?;

    if challenge_id.map(str::trim).unwrap_or("").is_empty() {
        return Err("challenge_id is required for binary join".into());
    }

    let snapshot = fetch_on_chain_join_snapshot(state, wager_id).await?;
    validate_binary_joinable_snapshot(&snapshot)?;

    let expected_key = game_id_key(game_id);
    if snapshot.game_id_key != expected_key {
        return Err("game_id does not match on-chain wager metadata".into());
    }

    if !intent.open {
        if let Some(token) = state.lichess_api_token.as_deref() {
            validate_challenge_destination(
                state,
                challenge_id.unwrap_or_default(),
                joiner_lichess_username,
                token,
            )
            .await?;
        }
    }

    Ok(())
}

pub async fn validate_competition_join(state: &AppState, wager_id: B256) -> Result<(), String> {
    let snapshot = fetch_on_chain_join_snapshot(state, wager_id).await?;
    validate_competition_joinable_snapshot(&snapshot)
}

#[derive(Debug)]
pub struct OnChainLeaveSnapshot {
    pub exists: bool,
    pub can_leave: bool,
}

pub async fn fetch_on_chain_leave_snapshot(
    state: &AppState,
    wager_id: B256,
    leaver: Address,
) -> Result<OnChainLeaveSnapshot, String> {
    let rpc_url = state
        .eth_rpc_url
        .as_deref()
        .ok_or_else(|| "ETH_RPC_URL is not configured".to_string())?;
    let contract = state
        .lichess_contract_address
        .ok_or_else(|| "LICHESS_CONTRACT_ADDRESS is not configured".to_string())?;

    let provider = ProviderBuilder::new().connect(rpc_url).await.map_err(|e| {
        format!("failed to connect RPC for leave validation: {e}")
    })?;
    let reader = LichessContractReader::new(contract, provider);
    let snapshot = reader
        .lcWagerLeaveSnapshot(wager_id, leaver)
        .call()
        .await
        .map_err(|e| format!("lcWagerLeaveSnapshot call failed: {e}"))?;

    Ok(OnChainLeaveSnapshot {
        exists: snapshot.exists,
        can_leave: snapshot.canLeave,
    })
}

pub fn validate_competition_leave_snapshot(snapshot: &OnChainLeaveSnapshot) -> Result<(), String> {
    if !snapshot.exists {
        return Err("wager does not exist on-chain".into());
    }
    if !snapshot.can_leave {
        return Err("wager is not open for leaving".into());
    }
    Ok(())
}

pub async fn validate_competition_leave(
    state: &AppState,
    wager_id: B256,
    leaver: Address,
) -> Result<(), String> {
    if leaver.is_zero() {
        return Err("leaver must not be the zero address".into());
    }
    let snapshot = fetch_on_chain_leave_snapshot(state, wager_id, leaver).await?;
    validate_competition_leave_snapshot(&snapshot)
}

async fn validate_challenge_destination(
    state: &AppState,
    challenge_id: &str,
    expected_username: &str,
    token: &str,
) -> Result<(), String> {
    let challenge_id = challenge_id.trim();
    if challenge_id.is_empty() {
        return Err("challenge_id is required".into());
    }

    let url = format!("{LICHESS_API_BASE}/api/challenge/{challenge_id}/show");
    let response = state
        .http
        .get(&url)
        .header(ACCEPT, "application/json")
        .header(AUTHORIZATION, format!("Bearer {token}"))
        .send()
        .await
        .map_err(|e| format!("lichess challenge request failed: {e}"))?;

    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Err(format!("lichess challenge `{challenge_id}` not found"));
    }
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!(
            "lichess challenge show failed ({status}): {}",
            body.chars().take(200).collect::<String>()
        ));
    }

    let data: Value = response
        .json()
        .await
        .map_err(|e| format!("lichess challenge response is not JSON: {e}"))?;

    let dest = extract_challenge_destination(&data);
    let expected = expected_username.trim().to_lowercase();
    match dest {
        Some(dest) if dest == expected => Ok(()),
        Some(dest) => Err(format!(
            "challenge destination `{dest}` does not match joiner `{expected}`"
        )),
        None => Err("could not resolve challenge destination username from Lichess".into()),
    }
}

fn extract_challenge_destination(data: &Value) -> Option<String> {
    if let Some(dest) = data.get("dest").and_then(|v| v.as_str()) {
        return Some(dest.trim().to_lowercase());
    }
    if let Some(name) = data
        .pointer("/destUser/name")
        .or_else(|| data.pointer("/destUser/id"))
        .and_then(|v| v.as_str())
    {
        return Some(name.trim().to_lowercase());
    }
    if let Some(name) = data
        .pointer("/opponent/name")
        .or_else(|| data.pointer("/opponent/id"))
        .and_then(|v| v.as_str())
    {
        return Some(name.trim().to_lowercase());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn game_id_key_matches_solidity_encode_packed() {
        let game_id = r#"{"v":1,"open":false,"opponent":"alice"}"#;
        let hash = game_id_key(game_id);
        assert_ne!(hash, B256::ZERO);
    }

    #[test]
    fn directed_opponent_must_match() {
        let intent = BinaryGameIntent {
            v: 1,
            open: false,
            opponent: Some("Alice".into()),
        };
        assert!(validate_binary_directed_opponent(&intent, "alice").is_ok());
        assert!(validate_binary_directed_opponent(&intent, "bob").is_err());
    }

    #[test]
    fn open_challenge_skips_opponent_match() {
        let intent = BinaryGameIntent {
            v: 1,
            open: true,
            opponent: None,
        };
        assert!(validate_binary_directed_opponent(&intent, "anyone").is_ok());
    }

    fn test_snapshot(
        format: u8,
        status: u8,
        participants_count: u64,
        expires_at: u64,
    ) -> OnChainJoinSnapshot {
        OnChainJoinSnapshot {
            exists: true,
            format,
            status,
            participants_count: U256::from(participants_count),
            expires_at: U256::from(expires_at),
            chain_now: U256::from(now_unix()),
            game_id_key: B256::ZERO,
        }
    }

    #[test]
    fn join_window_follows_chain_time_not_wall_clock() {
        let chain_now = 1_700_000_000u64;
        let expires_at = chain_now + 3600;
        assert!(
            now_unix() > expires_at,
            "fixture expiry must be in the wall-clock past"
        );
        let mut snap = test_snapshot(WAGER_FORMAT_BINARY, WG_FLAG_CREATED, 0, expires_at);
        snap.chain_now = U256::from(chain_now);
        assert!(validate_binary_joinable_snapshot(&snap).is_ok());
    }

    #[test]
    fn competition_allows_accepted_with_existing_joiners() {
        let future = now_unix() + 3600;
        let snap = test_snapshot(WAGER_FORMAT_MULTI_PARTY, WG_FLAG_ACCEPTED, 2, future);
        assert!(validate_competition_joinable_snapshot(&snap).is_ok());
    }

    #[test]
    fn competition_allows_created_with_no_joiners() {
        let future = now_unix() + 3600;
        let snap = test_snapshot(WAGER_FORMAT_MULTI_PARTY, WG_FLAG_CREATED, 0, future);
        assert!(validate_competition_joinable_snapshot(&snap).is_ok());
    }

    #[test]
    fn competition_rejects_expired_join_window() {
        let past = now_unix().saturating_sub(60);
        let snap = test_snapshot(WAGER_FORMAT_MULTI_PARTY, WG_FLAG_ACCEPTED, 1, past);
        assert_eq!(
            validate_competition_joinable_snapshot(&snap).unwrap_err(),
            "wager join window has expired"
        );
    }

    #[test]
    fn competition_rejects_terminal_status() {
        let future = now_unix() + 3600;
        for status in [WG_FLAG_CANCELLED, WG_FLAG_EXPIRED, WG_FLAG_COMPLETED] {
            let snap = test_snapshot(WAGER_FORMAT_MULTI_PARTY, status, 0, future);
            let err = validate_competition_joinable_snapshot(&snap).unwrap_err();
            assert!(err.contains("not open for joining"), "{err}");
        }
    }

    #[test]
    fn binary_rejects_accepted_status() {
        let future = now_unix() + 3600;
        let snap = test_snapshot(WAGER_FORMAT_BINARY, WG_FLAG_ACCEPTED, 1, future);
        let err = validate_binary_joinable_snapshot(&snap).unwrap_err();
        assert!(err.contains("not open for joining"), "{err}");
    }

    #[test]
    fn binary_rejects_when_joiner_already_present() {
        let future = now_unix() + 3600;
        let snap = test_snapshot(WAGER_FORMAT_BINARY, WG_FLAG_CREATED, 1, future);
        assert_eq!(
            validate_binary_joinable_snapshot(&snap).unwrap_err(),
            "wager already has a joiner"
        );
    }

    #[test]
    fn leave_rejects_when_not_eligible() {
        let snap = OnChainLeaveSnapshot {
            exists: true,
            can_leave: false,
        };
        assert_eq!(
            validate_competition_leave_snapshot(&snap).unwrap_err(),
            "wager is not open for leaving"
        );
    }

    #[test]
    fn leave_rejects_missing_wager() {
        let snap = OnChainLeaveSnapshot {
            exists: false,
            can_leave: false,
        };
        assert_eq!(
            validate_competition_leave_snapshot(&snap).unwrap_err(),
            "wager does not exist on-chain"
        );
    }
}
