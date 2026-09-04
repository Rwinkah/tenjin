use alloy::primitives::{Address, ChainId};
use alloy::signers::local::PrivateKeySigner;
use alloy::signers::Signer;
use reqwest::Client;
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use std::sync::Mutex;

use crate::shared::min_stake::PriceCache;

pub struct AppState {
    pub signer: PrivateKeySigner,
    pub http: Client,
    /// Address of the deployed `LichessOracle` contract on `chain_id`.
    /// The wager contract requires `attestation.oracle == oracle_address` and
    /// `isOracleContract[oracle_address] == true`.
    pub oracle_address: Address,
    /// Address of the deployed `FootballOracle` contract on `chain_id`.
    pub football_oracle_address: Address,
    /// Address of the deployed `StravaOracle` contract on `chain_id`.
    pub strava_oracle_address: Address,
    /// Address of the deployed `FplOracle` contract on `chain_id`.
    pub fpl_oracle_address: Address,
    /// football-data.org `X-Auth-Token` for fixture result verification.
    pub football_data_api_token: Option<String>,
    /// JSON-RPC URL for on-chain join validation (optional in dev; required for binary join).
    pub eth_rpc_url: Option<String>,
    /// Deployed `LichessContract` address (same as wager-backend).
    pub lichess_contract_address: Option<Address>,
    pub football_contract_address: Option<Address>,
    pub strava_contract_address: Option<Address>,
    pub fpl_contract_address: Option<Address>,
    /// Optional Lichess OAuth token for challenge destination verification on directed joins.
    pub lichess_api_token: Option<String>,
    pub db: Option<PgPool>,
    pub price_cache: Mutex<PriceCache>,
}

impl AppState {
    /// Builds state from environment:
    ///
    /// - `PRIVATE_KEY` — attester key allowlisted on oracles
    /// - `CHAIN_ID` — decimal u64
    /// - `LICHESS_ORACLE_ADDRESS` / `ORACLE_ADDRESS` — LichessOracle
    /// - `FOOTBALL_ORACLE_ADDRESS` — FootballOracle (optional; zero if unset)
    /// - `STRAVA_ORACLE_ADDRESS` — StravaOracle (optional; zero if unset)
    /// - `FPL_ORACLE_ADDRESS` — FplOracle (optional; zero if unset)
    /// - `FOOTBALL_DATA_API_TOKEN` (or legacy `API_FOOTBALL_KEY`) — football-data.org token
    /// - `ETH_RPC_URL`, `*_CONTRACT_ADDRESS`, `LICHESS_API_TOKEN` — join validation
    /// - `DATABASE_URL` — Postgres (campaign min-stake lookup; fail closed if unset)
    pub fn from_env() -> Self {
        let pk = std::env::var("PRIVATE_KEY").expect("PRIVATE_KEY must be set");
        let pk = pk.trim();

        let chain_id: ChainId = std::env::var("CHAIN_ID")
            .expect("CHAIN_ID must be set")
            .trim()
            .parse()
            .expect("CHAIN_ID must be a decimal u64");

        let signer: PrivateKeySigner = pk
            .parse()
            .expect("PRIVATE_KEY must be a valid 32-byte hex key (see PrivateKeySigner::from_str)");
        let signer = signer.with_chain_id(Some(chain_id));

        let oracle_raw = std::env::var("LICHESS_ORACLE_ADDRESS")
            .or_else(|_| std::env::var("ORACLE_ADDRESS"))
            .expect("LICHESS_ORACLE_ADDRESS must be set");
        let oracle_address: Address = oracle_raw
            .trim()
            .parse()
            .expect("LICHESS_ORACLE_ADDRESS must be a 0x-prefixed 20-byte address");

        let football_oracle_address: Address = std::env::var("FOOTBALL_ORACLE_ADDRESS")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .map(|s| s.parse().expect("FOOTBALL_ORACLE_ADDRESS must be a 0x address"))
            .unwrap_or(Address::ZERO);

        let strava_oracle_address: Address = std::env::var("STRAVA_ORACLE_ADDRESS")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .map(|s| s.parse().expect("STRAVA_ORACLE_ADDRESS must be a 0x address"))
            .unwrap_or(Address::ZERO);

        let fpl_oracle_address: Address = std::env::var("FPL_ORACLE_ADDRESS")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .map(|s| s.parse().expect("FPL_ORACLE_ADDRESS must be a 0x address"))
            .unwrap_or(Address::ZERO);

        let football_data_api_token = std::env::var("FOOTBALL_DATA_API_TOKEN")
            .or_else(|_| std::env::var("API_FOOTBALL_KEY"))
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());

        let http = Client::builder()
            .user_agent(concat!("tenjin-oracle/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("failed to build reqwest::Client");

        let eth_rpc_url = std::env::var("ETH_RPC_URL")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());

        let lichess_api_token = std::env::var("LICHESS_API_TOKEN")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());

        let db = std::env::var("DATABASE_URL")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .map(|url| {
                PgPoolOptions::new()
                    .max_connections(5)
                    .connect_lazy(&url)
                    .unwrap_or_else(|e| panic!("DATABASE_URL is invalid: {e}"))
            });

        Self {
            signer,
            http,
            oracle_address,
            football_oracle_address,
            strava_oracle_address,
            fpl_oracle_address,
            football_data_api_token,
            eth_rpc_url,
            lichess_contract_address: optional_address("LICHESS_CONTRACT_ADDRESS"),
            football_contract_address: optional_address("FOOTBALL_CONTRACT_ADDRESS"),
            strava_contract_address: optional_address("STRAVA_CONTRACT_ADDRESS"),
            fpl_contract_address: optional_address("FPL_CONTRACT_ADDRESS"),
            lichess_api_token,
            db,
            price_cache: Mutex::new(PriceCache::new()),
        }
    }
}

fn optional_address(var: &str) -> Option<Address> {
    std::env::var(var)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .map(|s| {
            s.parse::<Address>()
                .unwrap_or_else(|_| panic!("{var} must be a 0x-prefixed 20-byte address when set"))
        })
}
