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
    pub regular_time: Option<ScorePair>,
    pub half_time: Option<ScorePair>,
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

impl ScoreNode {
    fn pair_goals(pair: Option<&ScorePair>) -> Option<(i32, i32)> {
        let pair = pair?;
        Some((pair.home_goals()?, pair.away_goals()?))
    }

    pub fn settled_goals(&self) -> Option<(i32, i32)> {
        Self::pair_goals(self.full_time.as_ref())
            .or_else(|| Self::pair_goals(self.regular_time.as_ref()))
            .or_else(|| Self::pair_goals(self.half_time.as_ref()))
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
    /// Normalized short code used by `classify_fixture`.
    pub short: String,
    /// Original football-data.org / API-Football status string.
    pub raw: String,
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

/// Map football-data.org and API-Football status strings to short codes.
fn map_status_short(status: &str) -> String {
    match status.trim().to_uppercase().replace('-', "_").as_str() {
        "FINISHED" | "FT" | "FULL_TIME" => "FT".into(),
        "AET" | "FINISHED_AET" | "FULL_TIME_AET" => "AET".into(),
        "PEN" | "PENALTIES" | "PENALTY_SHOOTOUT" | "P" => "PEN".into(),
        "POSTPONED" | "PST" => "PST".into(),
        "CANCELLED" | "CANCELED" | "CANC" => "CANC".into(),
        "SUSPENDED" | "SUSP" => "SUSP".into(),
        "ABANDONED" | "ABD" => "ABD".into(),
        "AWARDED" | "AWD" | "WO" => "AWD".into(),
        "SCHEDULED" | "TIMED" | "NS" | "TBD" | "NOT_STARTED" => "NS".into(),
        "IN_PLAY" | "PAUSED" | "EXTRA_TIME" | "LIVE" | "1H" | "2H" | "HT" | "ET" | "BT"
        | "INT" => "LIVE".into(),
        other if other.is_empty() => "NS".into(),
        other => other.to_string(),
    }
}

fn outcome_from_goals(home: i32, away: i32) -> FixtureOutcomeClass {
    if home > away {
        FixtureOutcomeClass::HomeWin
    } else if away > home {
        FixtureOutcomeClass::AwayWin
    } else {
        FixtureOutcomeClass::Draw
    }
}

pub fn classify_fixture(item: &FixtureItem) -> FixtureOutcomeClass {
    let short = item.fixture.status.short.as_str();
    match short {
        "PST" | "CANC" | "ABD" | "AWD" | "WO" => return FixtureOutcomeClass::Void,
        "FT" | "AET" | "PEN" => {}
        "LIVE" => return FixtureOutcomeClass::NotReady,
        // Stale "not started" / unknown codes: if the feed already published a
        // full-time score, settle from that instead of blocking on status.
        _ => {
            let (Some(home), Some(away)) = (item.goals.home, item.goals.away) else {
                return FixtureOutcomeClass::NotReady;
            };
            return outcome_from_goals(home, away);
        }
    }

    let (Some(home), Some(away)) = (item.goals.home, item.goals.away) else {
        return FixtureOutcomeClass::NotReady;
    };
    outcome_from_goals(home, away)
}

pub fn fixture_status_label(item: &FixtureItem) -> String {
    let short = item.fixture.status.short.as_str();
    let raw = item.fixture.status.raw.as_str();
    if raw.is_empty() || raw.eq_ignore_ascii_case(short) {
        short.to_string()
    } else {
        format!("{short}, feed {raw}")
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

    let (goals_home, goals_away) = body
        .score
        .as_ref()
        .and_then(ScoreNode::settled_goals)
        .map(|(h, a)| (Some(h), Some(a)))
        .unwrap_or((None, None));

    Ok(FixtureItem {
        fixture: FixtureMeta {
            id: body.id,
            status: FixtureStatus {
                short: map_status_short(&body.status),
                raw: body.status,
            },
        },
        goals: Goals {
            home: goals_home,
            away: goals_away,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(status: &str, home: Option<i32>, away: Option<i32>) -> FixtureItem {
        FixtureItem {
            fixture: FixtureMeta {
                id: 1,
                status: FixtureStatus {
                    short: map_status_short(status),
                    raw: status.to_string(),
                },
            },
            goals: Goals { home, away },
        }
    }

    #[test]
    fn maps_finished_vocabularies_to_ft() {
        assert_eq!(map_status_short("FINISHED"), "FT");
        assert_eq!(map_status_short("FT"), "FT");
        assert_eq!(map_status_short("ft"), "FT");
    }

    #[test]
    fn does_not_collapse_unknown_status_to_ns() {
        assert_eq!(map_status_short("WEIRD"), "WEIRD");
    }

    #[test]
    fn finished_with_score_is_ready() {
        assert_eq!(
            classify_fixture(&item("FINISHED", Some(3), Some(0))),
            FixtureOutcomeClass::HomeWin
        );
        assert_eq!(
            classify_fixture(&item("FT", Some(1), Some(1))),
            FixtureOutcomeClass::Draw
        );
    }

    #[test]
    fn stale_ns_with_full_time_score_is_ready() {
        assert_eq!(
            classify_fixture(&item("NS", Some(2), Some(1))),
            FixtureOutcomeClass::HomeWin
        );
        assert_eq!(
            classify_fixture(&item("TIMED", Some(0), Some(0))),
            FixtureOutcomeClass::Draw
        );
    }

    #[test]
    fn ns_without_score_is_not_ready() {
        assert_eq!(
            classify_fixture(&item("NS", None, None)),
            FixtureOutcomeClass::NotReady
        );
    }

    #[test]
    fn live_is_not_ready_even_with_score() {
        assert_eq!(
            classify_fixture(&item("IN_PLAY", Some(1), Some(0))),
            FixtureOutcomeClass::NotReady
        );
    }
}
