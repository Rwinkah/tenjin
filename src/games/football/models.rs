use serde::{Deserialize, Serialize};

/// Markets supported for peer football wagers (matches FE/backend `kind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FootballMarketKind {
    #[serde(rename = "football_home_away")]
    HomeAway,
    #[serde(rename = "football_over_under")]
    OverUnder,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FootballGameId {
    pub v: u32,
    pub kind: FootballMarketKind,
    pub fixture_id: u64,
    pub league_id: Option<u64>,
    pub season: Option<u32>,
    pub home_team_id: Option<u64>,
    pub away_team_id: Option<u64>,
    pub home_name: Option<String>,
    pub away_name: Option<String>,
    pub kickoff: Option<String>,
    /// Over/under line; MVP expects 2.5.
    pub line: Option<f64>,
    pub creator_pick: String,
    pub joiner_pick: Option<String>,
    /// Missing `open` treated as true (legacy open challenges).
    #[serde(default = "default_open_true")]
    pub open: bool,
    /// Platform username (display_name), lowercased when directed.
    #[serde(default)]
    pub opponent: Option<String>,
    pub nonce: Option<String>,
}

fn default_open_true() -> bool {
    true
}

impl FootballGameId {
    pub fn parse(raw: &str) -> Result<Self, String> {
        serde_json::from_str(raw).map_err(|e| format!("invalid football gameId JSON: {e}"))
    }

    pub fn ensure_directed_joiner(&self, joiner_display_name: Option<&str>) -> Result<(), String> {
        if self.open {
            return Ok(());
        }
        let expected = self
            .opponent
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.to_ascii_lowercase())
            .ok_or_else(|| "directed wager missing opponent".to_string())?;
        let actual = joiner_display_name
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.to_ascii_lowercase())
            .ok_or_else(|| {
                "joiner has no username; set a display name before joining a directed wager"
                    .to_string()
            })?;
        if actual != expected {
            return Err(format!(
                "joiner username `{actual}` does not match directed opponent `{expected}`"
            ));
        }
        Ok(())
    }

    pub fn opposite_pick(pick: &str) -> Option<&'static str> {
        match pick {
            "1" => Some("2"),
            "2" => Some("1"),
            "O" | "o" => Some("U"),
            "U" | "u" => Some("O"),
            _ => None,
        }
    }

    pub fn normalize_pick(pick: &str) -> Option<String> {
        match pick {
            "1" | "2" => Some(pick.to_string()),
            "O" | "o" => Some("O".to_string()),
            "U" | "u" => Some("U".to_string()),
            _ => None,
        }
    }

    pub fn ensure_conflicting_picks(&self, joiner_pick: &str) -> Result<(String, String), String> {
        let creator = Self::normalize_pick(&self.creator_pick)
            .ok_or_else(|| format!("invalid creatorPick `{}`", self.creator_pick))?;
        let joiner = Self::normalize_pick(joiner_pick)
            .ok_or_else(|| format!("invalid joinerPick `{joiner_pick}`"))?;

        match self.kind {
            FootballMarketKind::HomeAway => {
                if creator != "1" && creator != "2" {
                    return Err("home_away creatorPick must be 1 or 2".into());
                }
                if joiner != "1" && joiner != "2" {
                    return Err("home_away joinerPick must be 1 or 2".into());
                }
            }
            FootballMarketKind::OverUnder => {
                if creator != "O" && creator != "U" {
                    return Err("over_under creatorPick must be O or U".into());
                }
                if joiner != "O" && joiner != "U" {
                    return Err("over_under joinerPick must be O or U".into());
                }
                let line = self.line.unwrap_or(2.5);
                if (line - 2.5).abs() > f64::EPSILON {
                    return Err("MVP only supports over/under line 2.5".into());
                }
            }
        }

        let expected = Self::opposite_pick(&creator).ok_or("unsupported creator pick")?;
        if joiner != expected {
            return Err(format!(
                "joinerPick `{joiner}` must be the opposite of creatorPick `{creator}` (expected `{expected}`)"
            ));
        }
        Ok((creator, joiner))
    }
}
