use alloy::primitives::{Address, ChainId};
use alloy::signers::local::PrivateKeySigner;
use alloy::signers::Signer;
use reqwest::Client;

pub struct AppState {
    pub signer: PrivateKeySigner,
    pub http: Client,
    /// Address of the deployed `LichessOracle` contract on `chain_id`.
    /// The wager contract requires `attestation.oracle == oracle_address` and
    /// `isOracleContract[oracle_address] == true`.
    pub oracle_address: Address,
}

impl AppState {
    /// Builds state from environment:
    ///
    /// - `PRIVATE_KEY` — 32-byte hex (with or without `0x`) for the oracle's `trustedSigner`.
    /// - `CHAIN_ID`    — decimal u64 of the chain where `LichessOracle` lives.
    /// - `LICHESS_ORACLE_ADDRESS` — 0x-prefixed `LichessOracle` contract (same as wager-backend / Render env group).
    /// - `ORACLE_ADDRESS` — deprecated alias for `LICHESS_ORACLE_ADDRESS`.
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

        let http = Client::builder()
            .user_agent(concat!("tenjin-oracle/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("failed to build reqwest::Client");

        Self {
            signer,
            http,
            oracle_address,
        }
    }
}
