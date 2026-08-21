use serde::{Deserialize, Serialize};

/// On-chain Strava `gameId` JSON (matches FE/backend rules blob).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StravaGameId {
    pub v: u32,
    #[serde(default)]
    pub start_at: Option<String>,
    #[serde(default)]
    pub grace_period_seconds: Option<u64>,
    #[serde(default)]
    pub activity_type: Option<String>,
    #[serde(default)]
    pub metric: Option<String>,
    #[serde(default)]
    pub compare: Option<String>,
    #[serde(default)]
    pub min_distance_meters: Option<f64>,
    /// Missing `open` treated as true (legacy open challenges).
    #[serde(default = "default_open_true")]
    pub open: bool,
    /// Platform username (display_name), lowercased when directed.
    #[serde(default)]
    pub opponent: Option<String>,
}

fn default_open_true() -> bool {
    true
}

impl StravaGameId {
    pub fn parse(raw: &str) -> Result<Self, String> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err("strava gameId is empty".into());
        }
        serde_json::from_str(trimmed).map_err(|e| format!("invalid strava gameId JSON: {e}"))
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
    fn parse_open_challenge() {
        let meta = StravaGameId::parse(
            r#"{"v":1,"start_at":"2026-08-21T00:00:00Z","grace_period_seconds":86400,"activity_type":"Run","metric":"distance","compare":"max"}"#,
        )
        .unwrap();
        assert!(meta.open);
        meta.ensure_directed_joiner(None).unwrap();
    }

    #[test]
    fn directed_requires_matching_username() {
        let meta = StravaGameId::parse(
            r#"{"v":1,"open":false,"opponent":"Ada","metric":"distance","compare":"max"}"#,
        )
        .unwrap();
        assert!(meta.ensure_directed_joiner(None).is_err());
        assert!(meta.ensure_directed_joiner(Some("bob")).is_err());
        meta.ensure_directed_joiner(Some("ADA")).unwrap();
    }
}
