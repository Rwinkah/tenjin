//! Campaign min-stake check before Tenjin signs a join permit.

use std::str::FromStr;
use std::time::{Duration, Instant};

use alloy::network::TransactionBuilder;
use alloy::primitives::{keccak256, Address, B256, Bytes, U256};
use alloy::providers::{Provider, ProviderBuilder};
use alloy::rpc::types::TransactionRequest;
use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use sqlx::PgPool;

use crate::shared::models::AppState;

const WEI_PER_ETH: f64 = 1e18;
const PRICE_TTL: Duration = Duration::from_secs(30);
const PLATFORM_SLUG: &str = "platform";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GameKind {
    Lichess,
    Football,
    Strava,
    Fpl,
}

impl GameKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Lichess => "lichess",
            Self::Football => "football",
            Self::Strava => "strava",
            Self::Fpl => "fpl",
        }
    }
}

#[derive(Debug, Clone)]
pub struct PriceCache {
    usd: Option<(Instant, f64)>,
    ngn: Option<(Instant, f64)>,
}

impl PriceCache {
    pub fn new() -> Self {
        Self { usd: None, ngn: None }
    }
}

#[derive(Debug)]
struct CampaignRow {
    slug: String,
    status: String,
    starts_at: Option<DateTime<Utc>>,
    ends_at: Option<DateTime<Utc>>,
    games: serde_json::Value,
    min_stake_currency: String,
    min_stake_amount: String,
    active: bool,
}

#[derive(Deserialize)]
struct CoinGeckoEth {
    ethereum: Option<CoinGeckoVs>,
}

#[derive(Deserialize)]
struct CoinGeckoVs {
    usd: Option<f64>,
    ngn: Option<f64>,
}

#[derive(Deserialize)]
struct DefiLlamaPrices {
    coins: Option<std::collections::HashMap<String, DefiLlamaCoin>>,
}

#[derive(Deserialize)]
struct DefiLlamaCoin {
    price: Option<f64>,
}

pub fn wei_to_eth(wei: U256) -> Result<f64, String> {
    let raw = wei.to_string();
    let value = f64::from_str(&raw).map_err(|_| format!("wei `{raw}` is not a number"))?;
    Ok(value / WEI_PER_ETH)
}

pub fn quoted_stake(eth: f64, currency: &str, usd: f64, ngn: f64) -> Result<f64, String> {
    match currency.to_ascii_uppercase().as_str() {
        "ETH" => Ok(eth),
        "USD" => {
            if usd <= 0.0 {
                return Err("ETH/USD rate is unavailable".into());
            }
            Ok(eth * usd)
        }
        "NGN" => {
            if ngn <= 0.0 {
                return Err("ETH/NGN rate is unavailable".into());
            }
            Ok(eth * ngn)
        }
        other => Err(format!("unsupported campaign currency `{other}`")),
    }
}

fn wagers_selector() -> [u8; 4] {
    let hash = keccak256(b"wagers(bytes32)");
    [hash[0], hash[1], hash[2], hash[3]]
}

pub async fn fetch_wg_min_amount(rpc_url: &str, contract: Address, wager_id: B256) -> Result<U256, String> {
    let provider = ProviderBuilder::new()
        .connect(rpc_url)
        .await
        .map_err(|e| format!("failed to connect RPC for min stake: {e}"))?;
    let mut data = Vec::with_capacity(36);
    data.extend_from_slice(&wagers_selector());
    data.extend_from_slice(wager_id.as_slice());
    let tx = TransactionRequest::default()
        .with_to(contract)
        .with_input(Bytes::from(data));
    let result = provider
        .call(tx)
        .await
        .map_err(|e| format!("wagers() call failed: {e}"))?;
    if result.len() < 160 {
        return Err(format!("wagers() return too short ({} bytes)", result.len()));
    }
    Ok(U256::from_be_slice(&result[128..160]))
}

async fn load_campaign(pool: &PgPool, slug: &str) -> Result<CampaignRow, String> {
    sqlx::query_as::<_, CampaignRow>(
        r#"
        SELECT slug, status, starts_at, ends_at, games, min_stake_currency,
               min_stake_amount::text AS min_stake_amount, active
        FROM campaigns_campaign
        WHERE slug = $1
        "#,
    )
    .bind(slug)
    .fetch_optional(pool)
    .await
    .map_err(|e| format!("campaign lookup failed: {e}"))?
    .ok_or_else(|| format!("unknown campaign `{slug}`"))
}

fn campaign_is_active(row: &CampaignRow) -> Result<(), String> {
    if !row.active || row.status != "active" {
        return Err(format!("campaign `{}` is not active", row.slug));
    }
    let now = Utc::now();
    if let Some(start) = row.starts_at {
        if now < start {
            return Err(format!("campaign `{}` has not started", row.slug));
        }
    }
    if let Some(end) = row.ends_at {
        if now > end {
            return Err(format!("campaign `{}` has ended", row.slug));
        }
    }
    Ok(())
}

fn campaign_allows_game(row: &CampaignRow, game: GameKind) -> Result<(), String> {
    match &row.games {
        serde_json::Value::Array(items) if !items.is_empty() => {
            let ok = items.iter().any(|item| item.as_str() == Some(game.as_str()));
            if ok {
                Ok(())
            } else {
                Err(format!(
                    "campaign `{}` does not apply to {}",
                    row.slug,
                    game.as_str()
                ))
            }
        }
        _ => Ok(()),
    }
}

async fn fetch_json<T: for<'de> Deserialize<'de>>(state: &AppState, url: &str) -> Option<T> {
    let response = state.http.get(url).send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    response.json().await.ok()
}

async fn fetch_eth_usd(state: &AppState) -> Result<f64, String> {
    if let Ok(cache) = state.price_cache.lock() {
        if let Some((at, price)) = cache.usd {
            if at.elapsed() < PRICE_TTL && price > 0.0 {
                return Ok(price);
            }
        }
    }
    let gecko: Option<CoinGeckoEth> = fetch_json(
        state,
        "https://api.coingecko.com/api/v3/simple/price?ids=ethereum&vs_currencies=usd",
    )
    .await;
    let price = gecko
        .and_then(|body| body.ethereum.and_then(|e| e.usd))
        .filter(|p| *p > 0.0);
    let price = match price {
        Some(p) => p,
        None => {
            let llama: DefiLlamaPrices = fetch_json(state, "https://coins.llama.fi/prices/current/coingecko:ethereum")
                .await
                .ok_or_else(|| "ETH/USD rate is unavailable".to_string())?;
            llama
                .coins
                .and_then(|c| c.get("coingecko:ethereum").and_then(|x| x.price))
                .filter(|p| *p > 0.0)
                .ok_or_else(|| "ETH/USD rate is unavailable".to_string())?
        }
    };
    if let Ok(mut cache) = state.price_cache.lock() {
        cache.usd = Some((Instant::now(), price));
    }
    Ok(price)
}

async fn fetch_eth_ngn(state: &AppState) -> Result<f64, String> {
    if let Ok(cache) = state.price_cache.lock() {
        if let Some((at, price)) = cache.ngn {
            if at.elapsed() < PRICE_TTL && price > 0.0 {
                return Ok(price);
            }
        }
    }
    let gecko: Option<CoinGeckoEth> = fetch_json(
        state,
        "https://api.coingecko.com/api/v3/simple/price?ids=ethereum&vs_currencies=ngn",
    )
    .await;
    let price = match gecko
        .and_then(|body| body.ethereum.and_then(|e| e.ngn))
        .filter(|p| *p > 0.0)
    {
        Some(p) => p,
        None => {
            let llama: DefiLlamaPrices = fetch_json(state, "https://coins.llama.fi/prices/current/coingecko:ethereum")
                .await
                .ok_or_else(|| "ETH/NGN rate is unavailable".to_string())?;
            llama
                .coins
                .and_then(|c| c.get("coingecko:ethereum").and_then(|x| x.price))
                .filter(|p| *p > 0.0)
                .map(|usd| usd * 1600.0)
                .ok_or_else(|| "ETH/NGN rate is unavailable".to_string())?
        }
    };
    if let Ok(mut cache) = state.price_cache.lock() {
        cache.ngn = Some((Instant::now(), price));
    }
    Ok(price)
}

fn contract_for_game(state: &AppState, game: GameKind) -> Result<Address, String> {
    let addr = match game {
        GameKind::Lichess => state.lichess_contract_address,
        GameKind::Football => state.football_contract_address,
        GameKind::Strava => state.strava_contract_address,
        GameKind::Fpl => state.fpl_contract_address,
    };
    addr.filter(|a| *a != Address::ZERO)
        .ok_or_else(|| format!("{} contract address is not configured", game.as_str()))
}

pub async fn enforce_min_stake(
    state: &AppState,
    campaign_slug: Option<&str>,
    game: GameKind,
    wager_id: B256,
) -> Result<(), (StatusCode, String)> {
    let pool = state.db.as_ref().ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "DATABASE_URL is not configured".to_string(),
    ))?;
    let rpc = state.eth_rpc_url.as_deref().ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "ETH_RPC_URL is not configured".to_string(),
    ))?;
    let slug = campaign_slug
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(PLATFORM_SLUG);

    let row = load_campaign(pool, slug)
        .await
        .map_err(|e| (StatusCode::FORBIDDEN, e))?;
    campaign_is_active(&row).map_err(|e| (StatusCode::FORBIDDEN, e))?;
    campaign_allows_game(&row, game).map_err(|e| (StatusCode::FORBIDDEN, e))?;

    let floor: f64 = row
        .min_stake_amount
        .parse()
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "campaign min stake is not a number".into()))?;
    if floor <= 0.0 {
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            "campaign min stake must be positive".into(),
        ));
    }

    let contract = contract_for_game(state, game).map_err(|e| (StatusCode::SERVICE_UNAVAILABLE, e))?;
    let wei = fetch_wg_min_amount(rpc, contract, wager_id)
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, e))?;
    let eth = wei_to_eth(wei).map_err(|e| (StatusCode::BAD_GATEWAY, e))?;

    let currency = row.min_stake_currency.to_ascii_uppercase();
    let usd = if currency == "USD" || currency == "NGN" {
        fetch_eth_usd(state).await.map_err(|e| (StatusCode::BAD_GATEWAY, e))?
    } else {
        0.0
    };
    let ngn = if currency == "NGN" {
        fetch_eth_ngn(state).await.map_err(|e| (StatusCode::BAD_GATEWAY, e))?
    } else {
        0.0
    };
    let quoted = quoted_stake(eth, &currency, usd, ngn).map_err(|e| (StatusCode::BAD_GATEWAY, e))?;
    if quoted + f64::EPSILON < floor {
        return Err((
            StatusCode::FORBIDDEN,
            format!(
                "Stake is below the {} minimum ({} {})",
                row.slug, row.min_stake_amount, row.min_stake_currency
            ),
        ));
    }
    Ok(())
}

impl sqlx::FromRow<'_, sqlx::postgres::PgRow> for CampaignRow {
    fn from_row(row: &sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        use sqlx::Row;
        Ok(Self {
            slug: row.try_get("slug")?,
            status: row.try_get("status")?,
            starts_at: row.try_get("starts_at")?,
            ends_at: row.try_get("ends_at")?,
            games: row.try_get("games")?,
            min_stake_currency: row.try_get("min_stake_currency")?,
            min_stake_amount: row.try_get("min_stake_amount")?,
            active: row.try_get("active")?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wei_one_eth() {
        let wei = U256::from(10).pow(U256::from(18));
        let eth = wei_to_eth(wei).unwrap();
        assert!((eth - 1.0).abs() < 1e-12);
    }

    #[test]
    fn usd_floor() {
        let quoted = quoted_stake(0.001, "USD", 2000.0, 0.0).unwrap();
        assert!((quoted - 2.0).abs() < 1e-9);
        assert!(quoted < 5.0);
        let ok = quoted_stake(0.01, "USD", 2000.0, 0.0).unwrap();
        assert!(ok >= 5.0);
    }

    #[test]
    fn ngn_and_eth() {
        let ngn = quoted_stake(1.0, "NGN", 0.0, 3_000_000.0).unwrap();
        assert!((ngn - 3_000_000.0).abs() < 1.0);
        let eth = quoted_stake(0.05, "ETH", 0.0, 0.0).unwrap();
        assert!((eth - 0.05).abs() < 1e-12);
    }

    fn sample_campaign(status: &str, games: serde_json::Value) -> CampaignRow {
        CampaignRow {
            slug: "platform".into(),
            status: status.into(),
            starts_at: None,
            ends_at: None,
            games,
            min_stake_currency: "USD".into(),
            min_stake_amount: "5".into(),
            active: true,
        }
    }

    #[test]
    fn inactive_campaign_is_rejected() {
        let row = sample_campaign("draft", serde_json::json!([]));
        assert!(campaign_is_active(&row).is_err());
    }

    #[test]
    fn game_allowlist() {
        let row = sample_campaign("active", serde_json::json!(["football"]));
        assert!(campaign_allows_game(&row, GameKind::Football).is_ok());
        assert!(campaign_allows_game(&row, GameKind::Lichess).is_err());
    }
}
