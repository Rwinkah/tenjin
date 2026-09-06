//! Lichess arena result verification for competition settlement attestations.

use std::collections::HashMap;

use alloy::primitives::Address;
use reqwest::header::ACCEPT;
use serde::Deserialize;

use crate::shared::models::AppState;

const LICHESS_API_BASE: &str = "https://lichess.org";

#[derive(Debug, Deserialize)]
struct ArenaResultRow {
    username: String,
    rank: u64,
}

#[derive(Debug, Clone)]
pub struct RosterEntry {
    pub wallet: Address,
    pub lichess_username: String,
}

#[derive(Debug, Clone)]
pub struct PlacementEntry {
    pub rank: u64,
    pub player: Address,
}

pub async fn fetch_arena_results(
    state: &AppState,
    arena_id: &str,
) -> Result<Vec<ArenaResultRow>, String> {
    let arena_id = arena_id.trim();
    if arena_id.is_empty() {
        return Err("lichess_event_id is required".into());
    }

    let url = format!("{LICHESS_API_BASE}/api/tournament/{arena_id}/results");
    let response = state
        .http
        .get(&url)
        .query(&[("nb", "100")])
        .header(ACCEPT, "application/x-ndjson")
        .send()
        .await
        .map_err(|e| format!("lichess arena results request failed: {e}"))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!(
            "lichess arena results failed ({status}): {}",
            body.chars().take(200).collect::<String>()
        ));
    }

    let mut results = Vec::new();
    for line in response.text().await.unwrap_or_default().lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let row: ArenaResultRow = serde_json::from_str(line)
            .map_err(|e| format!("lichess arena results line is not valid JSON: {e}"))?;
        results.push(row);
    }

    if results.is_empty() {
        return Err("lichess arena returned no results".into());
    }

    Ok(results)
}

fn rank_by_username(results: &[ArenaResultRow]) -> HashMap<String, u64> {
    results
        .iter()
        .map(|row| (row.username.trim().to_lowercase(), row.rank))
        .collect()
}

pub async fn validate_competition_placements(
    state: &AppState,
    lichess_event_id: &str,
    placements: &[PlacementEntry],
    roster: &[RosterEntry],
) -> Result<(), String> {
    if roster.is_empty() {
        return Err("roster must not be empty".into());
    }

    let results = fetch_arena_results(state, lichess_event_id).await?;
    let ranks = rank_by_username(&results);

    let wallet_to_username: HashMap<Address, String> = roster
        .iter()
        .map(|entry| (entry.wallet, entry.lichess_username.trim().to_lowercase()))
        .collect();

    for placement in placements {
        let username = wallet_to_username
            .get(&placement.player)
            .ok_or_else(|| format!("placement player {} is not in roster", placement.player))?;

        let lichess_rank = ranks.get(username).ok_or_else(|| {
            format!("roster player `{username}` not found in Lichess arena results")
        })?;

        if *lichess_rank != placement.rank {
            return Err(format!(
                "placement rank {} for `{username}` does not match Lichess rank {}",
                placement.rank, lichess_rank
            ));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rank_by_username_is_case_insensitive() {
        let results = vec![
            ArenaResultRow {
                username: "Alice".into(),
                rank: 1,
            },
            ArenaResultRow {
                username: "bob".into(),
                rank: 2,
            },
        ];
        let ranks = rank_by_username(&results);
        assert_eq!(ranks.get("alice"), Some(&1));
        assert_eq!(ranks.get("bob"), Some(&2));
    }
}
