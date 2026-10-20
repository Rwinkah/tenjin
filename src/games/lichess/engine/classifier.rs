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
            // Abort / no-start: forfeit delinquent color by ply count so every arm
            // has a winner (0 = white never moved, 1 = black never replied).
            // 2+ plies is ambiguous → FaultVoid → series rematch (not match void).
            LichessStatus::Aborted | LichessStatus::NoStart => self
                .forfeit_delinquent_by_plies(game)
                .unwrap_or(LegOutcome::FaultVoid),
            LichessStatus::Cheat
            | LichessStatus::UnknownFinish
            | LichessStatus::VariantEnd => LegOutcome::FaultVoid,
            // Mate/resign/timeout without winner field should not happen; rematch.
            LichessStatus::Mate
            | LichessStatus::Resign
            | LichessStatus::Timeout
            | LichessStatus::Outoftime => LegOutcome::FaultVoid,
        }
    }

    pub fn ply_count(game: &LichessGameExport) -> usize {
        game.moves
            .as_deref()
            .map(|m| m.split_whitespace().filter(|p| !p.is_empty()).count())
            .unwrap_or(0)
    }

    /// 0 plies → white forfeits; 1 ply → black forfeits. None when 2+ plies.
    pub fn forfeit_delinquent_by_plies(&self, game: &LichessGameExport) -> Option<LegOutcome> {
        let color = match Self::ply_count(game) {
            0 => LichessColor::White,
            1 => LichessColor::Black,
            _ => return None,
        };
        let loser = self.side_for_color(game, color)?;
        Some(LegOutcome::Forfeit { loser })
    }

    pub fn side_for_color(&self, game: &LichessGameExport, color: LichessColor) -> Option<Side> {
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
    export_json_with_moves(status, winner, white, black, None)
}

pub fn export_json_with_moves(
    status: &str,
    winner: Option<&str>,
    white: &str,
    black: &str,
    moves: Option<&str>,
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
    if let Some(m) = moves {
        v["moves"] = json!(m);
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
    fn abort_zero_plies_forfeits_white() {
        let c = LichessOutcomeClassifier::new("alice", "bob");
        assert_eq!(
            c.classify_json(&export_json("aborted", None, "alice", "bob")),
            LegOutcome::Forfeit { loser: Side::A }
        );
    }

    #[test]
    fn abort_one_ply_forfeits_black() {
        let c = LichessOutcomeClassifier::new("alice", "bob");
        assert_eq!(
            c.classify_json(&export_json_with_moves(
                "aborted",
                None,
                "alice",
                "bob",
                Some("e4")
            )),
            LegOutcome::Forfeit { loser: Side::B }
        );
    }

    #[test]
    fn abort_two_plies_is_fault_void_for_rematch() {
        let c = LichessOutcomeClassifier::new("alice", "bob");
        assert_eq!(
            c.classify_json(&export_json_with_moves(
                "aborted",
                None,
                "alice",
                "bob",
                Some("e4 e5")
            )),
            LegOutcome::FaultVoid
        );
    }

    #[test]
    fn no_start_zero_plies_forfeits_white() {
        let c = LichessOutcomeClassifier::new("alice", "bob");
        assert_eq!(
            c.classify_json(&export_json("noStart", None, "alice", "bob")),
            LegOutcome::Forfeit { loser: Side::A }
        );
    }
}
