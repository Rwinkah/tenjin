//! Premier League score fallback from the public FPL fixtures API.
//!
//! football-data.org fixture IDs are still the wager key. FPL is matched by
//! kickoff + team names and is only used when it marks the match ended.

use reqwest::header::ACCEPT;
use serde::Deserialize;

use crate::games::football::api_football::{FixtureItem, FixtureMeta, FixtureStatus, Goals};
use crate::games::football::models::FootballGameId;
use crate::shared::models::AppState;

const FPL_BASE: &str = "https://fantasy.premierleague.com/api";
const PREMIER_LEAGUE_ID: u64 = 2021;

#[derive(Debug, Clone, Deserialize)]
struct BootstrapPayload {
    #[serde(default)]
    teams: Vec<BootstrapTeam>,
}

#[derive(Debug, Clone, Deserialize)]
struct BootstrapTeam {
    id: u32,
    name: String,
    #[serde(default)]
    short_name: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct FplFixture {
    #[serde(default)]
    kickoff_time: Option<String>,
    #[serde(default)]
    finished: bool,
    #[serde(default)]
    finished_provisional: bool,
    team_h: u32,
    team_a: u32,
    #[serde(default)]
    team_h_score: Option<i32>,
    #[serde(default)]
    team_a_score: Option<i32>,
}

#[derive(Debug, Clone)]
pub struct FplLookup {
    pub home_name: String,
    pub away_name: String,
    pub kickoff: String,
}

impl FplLookup {
    pub fn from_game_id(meta: &FootballGameId) -> Option<Self> {
        let home_name = meta.home_name.as_deref()?.trim();
        let away_name = meta.away_name.as_deref()?.trim();
        let kickoff = meta.kickoff.as_deref()?.trim();
        if home_name.is_empty() || away_name.is_empty() || kickoff.is_empty() {
            return None;
        }
        Some(Self {
            home_name: home_name.to_string(),
            away_name: away_name.to_string(),
            kickoff: kickoff.to_string(),
        })
    }
}

pub fn is_premier_league(meta: &FootballGameId, competition_id: Option<u64>) -> bool {
    meta.league_id == Some(PREMIER_LEAGUE_ID) || competition_id == Some(PREMIER_LEAGUE_ID)
}

/// FPL only reports Premier League. Skip the extra calls for other comps.
pub fn should_try_fpl(meta: &FootballGameId, competition_id: Option<u64>) -> bool {
    FplLookup::from_game_id(meta).is_some()
        && (is_premier_league(meta, competition_id) || meta.league_id.is_none())
}

pub fn fpl_match_ended(finished: bool, finished_provisional: bool) -> bool {
    finished || finished_provisional
}

pub fn normalize_club_name(raw: &str) -> String {
    let mut s = raw.trim().to_ascii_lowercase();
    s = s.replace('&', " and ");
    s = s.replace('\'', "");
    s = s
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { ' ' })
        .collect::<String>();
    for alias in [
        ("nott m forest", "nottingham forest"),
        ("nottm forest", "nottingham forest"),
        ("notts forest", "nottingham forest"),
        ("man city", "manchester city"),
        ("man utd", "manchester united"),
        ("man united", "manchester united"),
        ("spurs", "tottenham"),
        ("wolves", "wolverhampton"),
    ] {
        if s == alias.0 {
            s = alias.1.to_string();
            break;
        }
    }
    let stripped = [
        "football club",
        " fc",
        " afc",
        " cf",
        " and hove albion",
        " wanderers",
        " hotspur",
        " united",
        " town",
        " city",
    ];
    let mut changed = true;
    while changed {
        changed = false;
        for suffix in stripped {
            if let Some(prefix) = s.strip_suffix(suffix) {
                let prefix = prefix.trim();
                if !prefix.is_empty() {
                    s = prefix.to_string();
                    changed = true;
                }
            }
        }
    }
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn clubs_match(left: &str, right: &str) -> bool {
    let a = normalize_club_name(left);
    let b = normalize_club_name(right);
    if a.is_empty() || b.is_empty() {
        return false;
    }
    if a == b {
        return true;
    }
    if a.len() >= 4 && b.len() >= 4 && (a.contains(&b) || b.contains(&a)) {
        return true;
    }
    false
}

pub fn kickoffs_match(left: &str, right: &str) -> bool {
    let a = normalize_kickoff(left);
    let b = normalize_kickoff(right);
    !a.is_empty() && a == b
}

fn normalize_kickoff(raw: &str) -> String {
    raw.trim()
        .replace("+00:00", "Z")
        .chars()
        .take(16)
        .collect()
}

pub fn ended_fixture_item(fixture_id: u64, home: i32, away: i32, source: &str) -> FixtureItem {
    FixtureItem {
        fixture: FixtureMeta {
            id: fixture_id,
            status: FixtureStatus {
                short: "FT".into(),
                raw: source.into(),
            },
        },
        goals: Goals {
            home: Some(home),
            away: Some(away),
        },
    }
}

pub async fn lookup_ended_fpl_score(
    state: &AppState,
    lookup: &FplLookup,
) -> Result<Option<(i32, i32)>, (axum::http::StatusCode, String)> {
    let bootstrap: BootstrapPayload = get_json(
        state,
        &format!("{FPL_BASE}/bootstrap-static/"),
        "FPL bootstrap",
    )
    .await?;
    let fixtures: Vec<FplFixture> =
        get_json(state, &format!("{FPL_BASE}/fixtures/"), "FPL fixtures").await?;

    let teams: std::collections::HashMap<u32, BootstrapTeam> =
        bootstrap.teams.into_iter().map(|t| (t.id, t)).collect();

    for fixture in fixtures {
        if !fpl_match_ended(fixture.finished, fixture.finished_provisional) {
            continue;
        }
        let (Some(home), Some(away)) = (fixture.team_h_score, fixture.team_a_score) else {
            continue;
        };
        let Some(kickoff) = fixture.kickoff_time.as_deref() else {
            continue;
        };
        if !kickoffs_match(kickoff, &lookup.kickoff) {
            continue;
        }
        let Some(home_team) = teams.get(&fixture.team_h) else {
            continue;
        };
        let Some(away_team) = teams.get(&fixture.team_a) else {
            continue;
        };
        let home_ok = clubs_match(&lookup.home_name, &home_team.name)
            || home_team
                .short_name
                .as_deref()
                .is_some_and(|short| clubs_match(&lookup.home_name, short));
        let away_ok = clubs_match(&lookup.away_name, &away_team.name)
            || away_team
                .short_name
                .as_deref()
                .is_some_and(|short| clubs_match(&lookup.away_name, short));
        if home_ok && away_ok {
            return Ok(Some((home, away)));
        }
    }
    Ok(None)
}

async fn get_json<T: for<'de> Deserialize<'de>>(
    state: &AppState,
    url: &str,
    label: &str,
) -> Result<T, (axum::http::StatusCode, String)> {
    let response = state
        .http
        .get(url)
        .header(ACCEPT, "application/json")
        .send()
        .await
        .map_err(|e| {
            (
                axum::http::StatusCode::BAD_GATEWAY,
                format!("{label} request failed: {e}"),
            )
        })?;
    let status = response.status();
    if !status.is_success() {
        return Err((
            axum::http::StatusCode::BAD_GATEWAY,
            format!("{label} returned HTTP {status}"),
        ));
    }
    response.json::<T>().await.map_err(|e| {
        (
            axum::http::StatusCode::BAD_GATEWAY,
            format!("{label} returned invalid JSON: {e}"),
        )
    })
}


pub async fn resolve_fixture(
    state: &AppState,
    meta: &FootballGameId,
) -> Result<FixtureItem, (axum::http::StatusCode, String)> {
    use crate::games::football::api_football::{classify_fixture, fetch_fixture, FixtureOutcomeClass};

    let fd = fetch_fixture(state, meta.fixture_id).await;
    match fd {
        Ok(item) => {
            if classify_fixture(&item) != FixtureOutcomeClass::NotReady {
                return Ok(item);
            }
            if let Some(resolved) = try_fpl_ended(state, meta).await {
                return Ok(resolved);
            }
            Ok(item)
        }
        Err(err) => {
            if let Some(resolved) = try_fpl_ended(state, meta).await {
                return Ok(resolved);
            }
            Err(err)
        }
    }
}

async fn try_fpl_ended(state: &AppState, meta: &FootballGameId) -> Option<FixtureItem> {
    if !should_try_fpl(meta, None) {
        return None;
    }
    let lookup = FplLookup::from_game_id(meta)?;
    match lookup_ended_fpl_score(state, &lookup).await {
        Ok(Some((home, away))) => Some(ended_fixture_item(meta.fixture_id, home, away, "FPL")),
        Ok(None) => None,
        Err(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_common_pl_aliases() {
        assert!(clubs_match("Liverpool FC", "Liverpool"));
        assert!(clubs_match("Nottingham Forest FC", "Nott'm Forest"));
        assert!(clubs_match("Manchester City", "Man City"));
        assert!(clubs_match("Manchester United FC", "Man Utd"));
        assert!(clubs_match("Tottenham Hotspur", "Spurs"));
        assert!(clubs_match("Wolverhampton Wanderers", "Wolves"));
        assert!(clubs_match("Brighton & Hove Albion", "Brighton"));
        assert!(!clubs_match("Liverpool", "Arsenal"));
    }

    #[test]
    fn kickoff_tolerates_zulu_vs_offset() {
        assert!(kickoffs_match(
            "2026-08-29T11:30:00Z",
            "2026-08-29T11:30:00+00:00"
        ));
        assert!(!kickoffs_match(
            "2026-08-29T11:30:00Z",
            "2026-08-29T14:00:00Z"
        ));
    }

    #[test]
    fn ended_requires_fpl_finished_flag() {
        assert!(fpl_match_ended(false, true));
        assert!(fpl_match_ended(true, false));
        assert!(!fpl_match_ended(false, false));
    }
}
