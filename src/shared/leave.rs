//! On-chain leave eligibility for Football, Strava and FPL competitions.
//!
//! Mirrors the Lichess check in `games::lichess::join`: read `<prefix>WagerLeaveSnapshot`
//! from the game contract and only sign a leave permit when the contract says the leaver can leave.

use std::time::{SystemTime, UNIX_EPOCH};

use alloy::primitives::{Address, B256};
use alloy::providers::ProviderBuilder;
use alloy::sol;
use axum::http::StatusCode;

use crate::shared::models::AppState;

/// Leave permits are used right away by the browser; keep the window short.
pub const LEAVE_PERMIT_TTL_SECS: u64 = 3600;

sol! {
    #[sol(rpc)]
    contract FootballLeaveReader {
        function fbWagerLeaveSnapshot(bytes32 wagerId, address leaver)
            external view returns (bool exists, bool canLeave);
    }

    #[sol(rpc)]
    contract StravaLeaveReader {
        function stWagerLeaveSnapshot(bytes32 wagerId, address leaver)
            external view returns (bool exists, bool canLeave);
    }

    #[sol(rpc)]
    contract FplLeaveReader {
        function fplWagerLeaveSnapshot(bytes32 wagerId, address leaver)
            external view returns (bool exists, bool canLeave);
    }
}

#[derive(Debug, Clone, Copy)]
pub enum LeaveGame {
    Football,
    Strava,
    Fpl,
}

impl LeaveGame {
    fn contract(self, state: &AppState) -> Result<Address, String> {
        let (address, env) = match self {
            LeaveGame::Football => (state.football_contract_address, "FOOTBALL_CONTRACT_ADDRESS"),
            LeaveGame::Strava => (state.strava_contract_address, "STRAVA_CONTRACT_ADDRESS"),
            LeaveGame::Fpl => (state.fpl_contract_address, "FPL_CONTRACT_ADDRESS"),
        };
        address.ok_or_else(|| format!("{env} is not configured"))
    }
}

pub fn leave_permit_deadline() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
        .saturating_add(LEAVE_PERMIT_TTL_SECS)
}

fn check_snapshot(exists: bool, can_leave: bool) -> Result<(), String> {
    if !exists {
        return Err("wager does not exist on-chain".into());
    }
    if !can_leave {
        return Err("wager is not open for leaving".into());
    }
    Ok(())
}

/// Ok when the contract reports `leaver` may leave `wager_id` right now.
pub async fn validate_leave(
    state: &AppState,
    game: LeaveGame,
    wager_id: B256,
    leaver: Address,
) -> Result<(), (StatusCode, String)> {
    if leaver.is_zero() {
        return Err((StatusCode::BAD_REQUEST, "leaver must not be the zero address".into()));
    }
    let internal = |e: String| (StatusCode::INTERNAL_SERVER_ERROR, e);
    let rpc_url = state
        .eth_rpc_url
        .as_deref()
        .ok_or_else(|| internal("ETH_RPC_URL is not configured".into()))?;
    let contract = game.contract(state).map_err(internal)?;
    let provider = ProviderBuilder::new()
        .connect(rpc_url)
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("failed to connect RPC for leave validation: {e}")))?;

    let rpc_err = |e: alloy::contract::Error| (StatusCode::BAD_GATEWAY, format!("leave snapshot call failed: {e}"));
    let (exists, can_leave) = match game {
        LeaveGame::Football => {
            let s = FootballLeaveReader::new(contract, provider)
                .fbWagerLeaveSnapshot(wager_id, leaver)
                .call()
                .await
                .map_err(rpc_err)?;
            (s.exists, s.canLeave)
        }
        LeaveGame::Strava => {
            let s = StravaLeaveReader::new(contract, provider)
                .stWagerLeaveSnapshot(wager_id, leaver)
                .call()
                .await
                .map_err(rpc_err)?;
            (s.exists, s.canLeave)
        }
        LeaveGame::Fpl => {
            let s = FplLeaveReader::new(contract, provider)
                .fplWagerLeaveSnapshot(wager_id, leaver)
                .call()
                .await
                .map_err(rpc_err)?;
            (s.exists, s.canLeave)
        }
    };

    check_snapshot(exists, can_leave).map_err(|reason| (StatusCode::FORBIDDEN, reason))
}

#[cfg(test)]
mod tests {
    use super::check_snapshot;

    #[test]
    fn rejects_missing_or_closed_wagers() {
        assert!(check_snapshot(true, true).is_ok());
        assert!(check_snapshot(false, false).is_err());
        assert!(check_snapshot(true, false).is_err());
    }
}
