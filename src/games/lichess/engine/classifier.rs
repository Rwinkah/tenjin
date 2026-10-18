//! Map Lichess game export → shared [`LegOutcome`].

use serde_json::{Value, json};

use crate::shared::engine::traits::OutcomeClassifier;
use crate::shared::engine::{LegOutcome, Side};

use crate::games::lichess::models::{LichessColor, LichessGameExport, LichessStatus};

/// Classifies Lichess exports. Side mapping uses lowercase usernames for white/black.
#[derive(Debug, Clone)]
pub struct LichessOutcomeClassifier {
    pub side_a_username: String,
    pub side_b_username: String,
}

impl LichessOutcomeClassifier {
    pub fn new(side_a_username: impl Into<String>, side_b_username: impl Into<String>) -> Self {
        Self {
            side_a_username: side_a_username.into().to_ascii_lowercase(),
            side_b_username: side_b_username.into().to_ascii_lowercase(),
        }
    }

    pub fn classify_export(&self, game: &LichessGameExport) -> LegOutcome {
        if let Some(color) = game.winner {
            return match self.side_for_color(game, color) {
                Some(side) => LegOutcome::Decisive { winner: side },
                None => LegOutcome::FaultVoid,
            };
        }

        match game.status {
            LichessStatus::Created | LichessStatus::Started => LegOutcome::Ongoing,
            LichessStatus::Draw | LichessStatus::Stalemate => LegOutcome::Draw,
            // Ambiguous abort/no-start → FaultVoid. Series transition maps this to
            // match void when abandon.unilateral_scope is entire_match (anti-rematch).
            LichessStatus::Aborted
            | LichessStatus::NoStart
            | LichessStatus::Cheat
            | LichessStatus::UnknownFinish
            | LichessStatus::VariantEnd => LegOutcome::FaultVoid,
            // Mate/resign/timeout without winner field should not happen; treat as void-replay.
            LichessStatus::Mate
            | LichessStatus::Resign
            | LichessStatus::Timeout
            | LichessStatus::Outoftime => LegOutcome::FaultVoid,
        }
    }

    fn side_for_color(&self, game: &LichessGameExport, color: LichessColor) -> Option<Side> {
        let white = game
            .players
            .white
            .user
            .as_ref()
            .map(|u| u.id.to_ascii_lowercase())
            .unwrap_or_default();
        let black = game
            .players
            .black
            .user
            .as_ref()
            .map(|u| u.id.to_ascii_lowercase())
            .unwrap_or_default();
        let winner_name = match color {
            LichessColor::White => white,
            LichessColor::Black => black,
        };
        if winner_name == self.side_a_username {
            Some(Side::A)
        } else if winner_name == self.side_b_username {
            Some(Side::B)
        } else {
            None
        }
    }

    /// Classify from JSON (export-shaped or `{ "status", "winner", "players" }`).
    pub fn classify_json(&self, raw: &Value) -> LegOutcome {
        match serde_json::from_value::<LichessGameExport>(raw.clone()) {
            Ok(game) => self.classify_export(&game),
            Err(_) => {
                // Minimal fallback for tests / partial payloads.
                let status = raw
                    .get("status")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_ascii_lowercase();
                let winner = raw.get("winner").and_then(|v| v.as_str());
                if let Some(w) = winner {
                    let side = match w.to_ascii_lowercase().as_str() {
                        "a" | "side_a" => Some(Side::A),
                        "b" | "side_b" => Some(Side::B),
                        "white" => Some(Side::A), // only valid if side_a is white — tests may set explicitly
                        "black" => Some(Side::B),
                        _ => None,
                    };
                    if let Some(side) = side {
                        return LegOutcome::Decisive { winner: side };
                    }
                }
                match status.as_str() {
                    "created" | "started" | "" => LegOutcome::Ongoing,
                    "draw" | "stalemate" => LegOutcome::Draw,
                    "forfeit_a" => LegOutcome::Forfeit { loser: Side::A },
                    "forfeit_b" => LegOutcome::Forfeit { loser: Side::B },
                    "mutual_void" => LegOutcome::MutualVoid,
                    _ => LegOutcome::FaultVoid,
                }
            }
        }
    }
}

impl OutcomeClassifier for LichessOutcomeClassifier {
    fn classify_value(&self, raw: &Value) -> LegOutcome {
        self.classify_json(raw)
    }
}

/// Helper for building a minimal export-like JSON in tests.
pub fn export_json(
    status: &str,
    winner: Option<&str>,
    white: &str,
    black: &str,
) -> Value {
    let mut v = json!({
        "id": "testhash",
        "status": status,
        "players": {
            "white": { "user": { "id": white } },
            "black": { "user": { "id": black } }
        }
    });
    if let Some(w) = winner {
        v["winner"] = json!(w);
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decisive_maps_white_to_side_a() {
        let c = LichessOutcomeClassifier::new("alice", "bob");
        let raw = export_json("mate", Some("white"), "alice", "bob");
        assert_eq!(
            c.classify_json(&raw),
            LegOutcome::Decisive { winner: Side::A }
        );
    }

    #[test]
    fn draw_and_ongoing() {
        let c = LichessOutcomeClassifier::new("alice", "bob");
        assert_eq!(
            c.classify_json(&export_json("draw", None, "alice", "bob")),
            LegOutcome::Draw
        );
        assert_eq!(
            c.classify_json(&export_json("started", None, "alice", "bob")),
            LegOutcome::Ongoing
        );
    }

    #[test]
    fn abort_is_fault_void_without_fault_hint() {
        let c = LichessOutcomeClassifier::new("alice", "bob");
        assert_eq!(
            c.classify_json(&export_json("aborted", None, "alice", "bob")),
            LegOutcome::FaultVoid
        );
    }
}
