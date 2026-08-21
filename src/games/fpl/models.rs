use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FplMarketKind {
    #[serde(rename = "fpl_gameweek")]
    Gameweek,
    #[serde(rename = "fpl_league")]
    League,
    #[serde(rename = "fpl_league_pot")]
    LeaguePot,
    #[serde(rename = "fpl_h2h")]
    H2h,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FplGameId {
    pub v: u32,
    pub kind: FplMarketKind,
    #[serde(default)]
    pub event: Option<u32>,
    #[serde(default)]
    pub settle_event: Option<u32>,
    #[serde(default)]
    pub league_id: Option<u64>,
    #[serde(default)]
    pub h2h_match_code: Option<String>,
    pub creator_entry_id: u64,
    #[serde(default)]
    pub joiner_entry_id: Option<u64>,
    #[serde(default = "default_open_true")]
    pub open: bool,
    #[serde(default)]
    pub opponent: Option<String>,
    #[serde(default)]
    pub stake_mode: Option<String>,
    #[serde(default)]
    pub nonce: Option<String>,
}

fn default_open_true() -> bool {
    true
}

impl FplGameId {
    pub fn parse(raw: &str) -> Result<Self, String> {
        serde_json::from_str(raw).map_err(|e| format!("invalid FPL gameId JSON: {e}"))
    }

    pub fn settle_event_id(&self) -> Result<u32, String> {
        match self.kind {
            FplMarketKind::Gameweek | FplMarketKind::H2h => self
                .event
                .ok_or_else(|| "gameId missing event".to_string()),
            FplMarketKind::League | FplMarketKind::LeaguePot => self
                .settle_event
                .or(self.event)
                .ok_or_else(|| "gameId missing settleEvent".to_string()),
        }
    }

    pub fn join_event_id(&self) -> Result<u32, String> {
        self.settle_event_id()
    }

    pub fn requires_league(&self) -> bool {
        matches!(
            self.kind,
            FplMarketKind::League | FplMarketKind::LeaguePot | FplMarketKind::H2h
        )
    }

    pub fn league_id_required(&self) -> Result<u64, String> {
        self.league_id
            .filter(|id| *id > 0)
            .ok_or_else(|| "gameId missing leagueId".to_string())
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_gameweek() {
        let raw = r#"{"v":1,"kind":"fpl_gameweek","event":1,"creatorEntryId":4106192,"open":true}"#;
        let meta = FplGameId::parse(raw).unwrap();
        assert_eq!(meta.kind, FplMarketKind::Gameweek);
        assert_eq!(meta.settle_event_id().unwrap(), 1);
    }
}
