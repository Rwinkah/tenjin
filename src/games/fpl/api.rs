//! Fantasy Premier League public API client.

use reqwest::header::ACCEPT;
use serde::Deserialize;

use crate::shared::models::AppState;

const FPL_BASE: &str = "https://fantasy.premierleague.com/api";

#[derive(Debug, Clone, Deserialize)]
pub struct BootstrapPayload {
    #[serde(default)]
    pub events: Vec<BootstrapEvent>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BootstrapEvent {
    pub id: u32,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub deadline_time: Option<String>,
    #[serde(default)]
    pub finished: bool,
    #[serde(default)]
    pub data_checked: bool,
    #[serde(default)]
    pub is_current: bool,
    #[serde(default)]
    pub is_next: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EntryPayload {
    pub id: Option<u64>,
    #[serde(default)]
    pub started_event: Option<u32>,
    #[serde(default)]
    pub leagues: Option<EntryLeagues>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EntryLeagues {
    #[serde(default)]
    pub classic: Vec<LeagueRef>,
    #[serde(default)]
    pub h2h: Vec<LeagueRef>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LeagueRef {
    pub id: u64,
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PicksPayload {
    #[serde(default)]
    pub entry_history: Option<EntryHistoryRow>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EntryHistoryRow {
    #[serde(default)]
    pub event: Option<u32>,
    #[serde(default)]
    pub points: Option<i32>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ClassicStandingsPayload {
    #[serde(default)]
    pub standings: Option<StandingsPage>,
    #[serde(default)]
    pub league: Option<LeagueMeta>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LeagueMeta {
    pub id: Option<u64>,
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct StandingsPage {
    #[serde(default)]
    pub has_next: bool,
    #[serde(default)]
    pub results: Vec<StandingRow>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct StandingRow {
    pub entry: u64,
    #[serde(default)]
    pub rank: Option<u32>,
    #[serde(default)]
    pub total: Option<i32>,
    #[serde(default)]
    pub entry_name: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct H2hMatchesPayload {
    #[serde(default)]
    pub has_next: bool,
    #[serde(default)]
    pub results: Vec<H2hMatchRow>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct H2hMatchRow {
    #[serde(default)]
    pub event: Option<u32>,
    #[serde(default)]
    pub entry_1_entry: Option<u64>,
    #[serde(default)]
    pub entry_2_entry: Option<u64>,
    #[serde(default)]
    pub entry_1_points: Option<i32>,
    #[serde(default)]
    pub entry_2_points: Option<i32>,
}

fn map_err_status(status: reqwest::StatusCode, body: String, label: &str) -> (axum::http::StatusCode, String) {
    let code = if status.as_u16() == 404 {
        axum::http::StatusCode::NOT_FOUND
    } else {
        axum::http::StatusCode::BAD_GATEWAY
    };
    (
        code,
        format!(
            "{label} failed ({status}): {}",
            body.chars().take(200).collect::<String>()
        ),
    )
}

async fn get_json<T: for<'de> Deserialize<'de>>(
    state: &AppState,
    url: &str,
    query: &[(&str, String)],
    label: &str,
) -> Result<T, (axum::http::StatusCode, String)> {
    let mut request = state.http.get(url).header(ACCEPT, "application/json");
    if !query.is_empty() {
        request = request.query(query);
    }
    let response = request.send().await.map_err(|e| {
        (
            axum::http::StatusCode::BAD_GATEWAY,
            format!("{label} request failed: {e}"),
        )
    })?;
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(map_err_status(status, body, label));
    }
    response.json::<T>().await.map_err(|e| {
        (
            axum::http::StatusCode::BAD_GATEWAY,
            format!("{label} returned invalid JSON: {e}"),
        )
    })
}

pub async fn fetch_bootstrap(state: &AppState) -> Result<BootstrapPayload, (axum::http::StatusCode, String)> {
    get_json(state, &format!("{FPL_BASE}/bootstrap-static/"), &[], "FPL bootstrap").await
}

pub fn event_by_id<'a>(bootstrap: &'a BootstrapPayload, event_id: u32) -> Option<&'a BootstrapEvent> {
    bootstrap.events.iter().find(|event| event.id == event_id)
}

pub fn event_is_data_checked(bootstrap: &BootstrapPayload, event_id: u32) -> bool {
    event_by_id(bootstrap, event_id)
        .map(|event| event.data_checked)
        .unwrap_or(false)
}

/// `None` if the bootstrap payload has no row for this gameweek.
pub fn event_is_finished(bootstrap: &BootstrapPayload, event_id: u32) -> Option<bool> {
    event_by_id(bootstrap, event_id).map(|event| event.finished)
}

pub async fn fetch_entry(
    state: &AppState,
    entry_id: u64,
) -> Result<EntryPayload, (axum::http::StatusCode, String)> {
    get_json(
        state,
        &format!("{FPL_BASE}/entry/{entry_id}/"),
        &[],
        "FPL entry",
    )
    .await
}

pub async fn fetch_event_picks(
    state: &AppState,
    entry_id: u64,
    event: u32,
) -> Result<Option<PicksPayload>, (axum::http::StatusCode, String)> {
    let url = format!("{FPL_BASE}/entry/{entry_id}/event/{event}/picks/");
    let response = state
        .http
        .get(&url)
        .header(ACCEPT, "application/json")
        .send()
        .await
        .map_err(|e| {
            (
                axum::http::StatusCode::BAD_GATEWAY,
                format!("FPL picks request failed: {e}"),
            )
        })?;
    if response.status().as_u16() == 404 {
        return Ok(None);
    }
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(map_err_status(status, body, "FPL picks"));
    }
    let payload = response.json::<PicksPayload>().await.map_err(|e| {
        (
            axum::http::StatusCode::BAD_GATEWAY,
            format!("FPL picks returned invalid JSON: {e}"),
        )
    })?;
    Ok(Some(payload))
}

pub fn gw_points(picks: &PicksPayload) -> Option<i32> {
    picks.entry_history.as_ref().and_then(|row| row.points)
}

pub async fn fetch_gw_points(
    state: &AppState,
    entry_id: u64,
    event: u32,
) -> Result<Option<i32>, (axum::http::StatusCode, String)> {
    Ok(fetch_event_picks(state, entry_id, event)
        .await?
        .and_then(|picks| gw_points(&picks)))
}

pub async fn fetch_classic_standing(
    state: &AppState,
    league_id: u64,
    entry_id: u64,
) -> Result<Option<StandingRow>, (axum::http::StatusCode, String)> {
    let mut found = fetch_classic_standings(state, league_id, &[entry_id]).await?;
    Ok(found.remove(&entry_id))
}

pub async fn fetch_classic_standings(
    state: &AppState,
    league_id: u64,
    entry_ids: &[u64],
) -> Result<std::collections::HashMap<u64, StandingRow>, (axum::http::StatusCode, String)> {
    use std::collections::{HashMap, HashSet};
    let wanted: HashSet<u64> = entry_ids.iter().copied().filter(|id| *id > 0).collect();
    let mut found = HashMap::new();
    if wanted.is_empty() {
        return Ok(found);
    }
    let mut page: u32 = 1;
    while page <= 40 && found.len() < wanted.len() {
        let payload: ClassicStandingsPayload = get_json(
            state,
            &format!("{FPL_BASE}/leagues-classic/{league_id}/standings/"),
            &[("page", page.to_string())],
            "FPL classic standings",
        )
        .await?;
        let standings = payload.standings.unwrap_or(StandingsPage {
            has_next: false,
            results: vec![],
        });
        for row in standings.results {
            if wanted.contains(&row.entry) {
                found.insert(row.entry, row);
            }
        }
        if !standings.has_next {
            break;
        }
        page += 1;
    }
    Ok(found)
}

pub async fn fetch_h2h_match(
    state: &AppState,
    league_id: u64,
    event: u32,
    entry_a: u64,
    entry_b: u64,
) -> Result<Option<H2hMatchRow>, (axum::http::StatusCode, String)> {
    let wanted = [entry_a, entry_b];
    let mut page: u32 = 1;
    while page <= 20 {
        let payload: H2hMatchesPayload = get_json(
            state,
            &format!("{FPL_BASE}/leagues-h2h-matches/league/{league_id}/"),
            &[
                ("page", page.to_string()),
                ("event", event.to_string()),
            ],
            "FPL H2H matches",
        )
        .await?;
        for row in &payload.results {
            let left = row.entry_1_entry.unwrap_or(0);
            let right = row.entry_2_entry.unwrap_or(0);
            if wanted.contains(&left) && wanted.contains(&right) {
                return Ok(Some(row.clone()));
            }
        }
        if !payload.has_next {
            break;
        }
        page += 1;
    }
    Ok(None)
}

pub fn entry_in_classic_league(entry: &EntryPayload, league_id: u64) -> bool {
    entry
        .leagues
        .as_ref()
        .map(|leagues| leagues.classic.iter().any(|row| row.id == league_id))
        .unwrap_or(false)
}

pub fn entry_in_h2h_league(entry: &EntryPayload, league_id: u64) -> bool {
    entry
        .leagues
        .as_ref()
        .map(|leagues| leagues.h2h.iter().any(|row| row.id == league_id))
        .unwrap_or(false)
}
