//! football-data.org client for settle verification.

use reqwest::header::{HeaderMap, HeaderValue};
use serde::Deserialize;

use crate::shared::models::AppState;

const API_BASE: &str = "https://api.football-data.org/v4";

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MatchResponse {
    pub id: u64,
    pub status: String,
    #[allow(dead_code)]
    pub utc_date: Option<String>,
    pub score: Option<ScoreNode>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScoreNode {
    pub full_time: Option<ScorePair>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ScorePair {
    #[serde(default)]
    pub home: Option<i32>,
    #[serde(default)]
    pub away: Option<i32>,
    #[serde(default, rename = "homeTeam")]
    pub home_team: Option<i32>,
    #[serde(default, rename = "awayTeam")]
    pub away_team: Option<i32>,
}

impl ScorePair {
    pub fn home_goals(&self) -> Option<i32> {
        self.home.or(self.home_team)
    }

    pub fn away_goals(&self) -> Option<i32> {
        self.away.or(self.away_team)
    }
}

/// Adapter kept for existing settle handlers (API-Football-shaped fields).
#[derive(Debug, Clone)]
pub struct FixtureItem {
    pub fixture: FixtureMeta,
    pub goals: Goals,
}

#[derive(Debug, Clone)]
pub struct FixtureMeta {
    #[allow(dead_code)]
    pub id: u64,
    pub status: FixtureStatus,
}

#[derive(Debug, Clone)]
pub struct FixtureStatus {
    pub short: String,
}

#[derive(Debug, Clone)]
pub struct Goals {
    pub home: Option<i32>,
    pub away: Option<i32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FixtureOutcomeClass {
    HomeWin,
    AwayWin,
    Draw,
    Void,
    NotReady,
}

/// Map football-data.org status → short codes understood by classify_fixture.
fn map_status_short(status: &str) -> &'static str {
    match status.to_uppercase().as_str() {
        "FINISHED" => "FT",
        "POSTPONED" => "PST",
        "CANCELLED" => "CANC",
        "SUSPENDED" => "SUSP",
        "AWARDED" => "AWD",
        "SCHEDULED" | "TIMED" => "NS",
        "IN_PLAY" | "PAUSED" | "EXTRA_TIME" | "PENALTY_SHOOTOUT" => "LIVE",
        _ => "NS",
    }
}

pub fn classify_fixture(item: &FixtureItem) -> FixtureOutcomeClass {
    let short = item.fixture.status.short.as_str();
    match short {
        "PST" | "CANC" | "ABD" | "AWD" | "WO" => return FixtureOutcomeClass::Void,
        "FT" | "AET" | "PEN" => {}
        _ => return FixtureOutcomeClass::NotReady,
    }

    let (Some(home), Some(away)) = (item.goals.home, item.goals.away) else {
        return FixtureOutcomeClass::NotReady;
    };
    if home > away {
        FixtureOutcomeClass::HomeWin
    } else if away > home {
        FixtureOutcomeClass::AwayWin
    } else {
        FixtureOutcomeClass::Draw
    }
}

pub async fn fetch_fixture(
    state: &AppState,
    fixture_id: u64,
) -> Result<FixtureItem, (axum::http::StatusCode, String)> {
    let key = state
        .football_data_api_token
        .as_deref()
        .filter(|k| !k.is_empty())
        .ok_or((
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            "FOOTBALL_DATA_API_TOKEN is not configured".into(),
        ))?;

    let mut headers = HeaderMap::new();
    headers.insert(
        "X-Auth-Token",
        HeaderValue::from_str(key).map_err(|_| {
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "invalid FOOTBALL_DATA_API_TOKEN".into(),
            )
        })?,
    );

    let url = format!("{API_BASE}/matches/{fixture_id}");
    let response = state
        .http
        .get(&url)
        .headers(headers)
        .send()
        .await
        .map_err(|e| {
            (
                axum::http::StatusCode::BAD_GATEWAY,
                format!("football-data.org request failed: {e}"),
            )
        })?;

    let status = response.status();
    if status == reqwest::StatusCode::NOT_FOUND {
        return Err((
            axum::http::StatusCode::NOT_FOUND,
            format!("fixture `{fixture_id}` not found"),
        ));
    }
    if !status.is_success() {
        return Err((
            axum::http::StatusCode::BAD_GATEWAY,
            format!("football-data.org returned HTTP {status}"),
        ));
    }

    let body = response.json::<MatchResponse>().await.map_err(|e| {
        (
            axum::http::StatusCode::BAD_GATEWAY,
            format!("invalid JSON from football-data.org: {e}"),
        )
    })?;

    let full_time = body.score.as_ref().and_then(|s| s.full_time.as_ref());
    let goals_home = full_time.and_then(|s| s.home_goals());
    let goals_away = full_time.and_then(|s| s.away_goals());

    Ok(FixtureItem {
        fixture: FixtureMeta {
            id: body.id,
            status: FixtureStatus {
                short: map_status_short(&body.status).to_string(),
            },
        },
        goals: Goals {
            home: goals_home,
            away: goals_away,
        },
    })
}
