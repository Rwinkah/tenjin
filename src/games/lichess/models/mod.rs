use serde::{Deserialize, Serialize};

// ---------- Lichess "export one game" JSON shape (the subset we care about) ----------

/// Top-level response from `GET https://lichess.org/game/export/{gameId}` with
/// `Accept: application/json`. Only fields needed for oracle settlement are modeled.
#[derive(Debug, Deserialize)]
pub struct LichessGameExport {
    pub id: String,
    pub status: LichessStatus,
    pub winner: Option<LichessColor>,
    pub players: LichessPlayers,
}

/// The values Lichess emits in the `status` field.
///
/// Lichess uses camelCase for compound values like `noStart` / `unknownFinish` / `variantEnd`.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum LichessStatus {
    Created,
    Started,
    Aborted,
    Mate,
    Resign,
    Stalemate,
    Timeout,
    Draw,
    Outoftime,
    Cheat,
    NoStart,
    UnknownFinish,
    VariantEnd,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum LichessColor {
    White,
    Black,
}

#[derive(Debug, Deserialize)]
pub struct LichessPlayers {
    pub white: LichessPlayer,
    pub black: LichessPlayer,
}

#[derive(Debug, Deserialize)]
pub struct LichessPlayer {
    /// `None` for anonymous players (e.g. some bots / Stockfish challenges).
    pub user: Option<LichessUser>,
}

#[derive(Debug, Deserialize)]
pub struct LichessUser {
    pub id: String,
}

