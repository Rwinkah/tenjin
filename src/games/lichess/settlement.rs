//! Lichess export classification for 1v1 wager attestations.
//!
//! Aligns with frontend `classifyLichessSettlement`: void when the game is
//! terminal without a white/black winner and is not a draw.

use axum::http::StatusCode;

use crate::games::lichess::models::{LichessGameExport, LichessStatus};

/// Status values that indicate void when there is no white/black winner.
pub fn is_void_lichess_status(status: LichessStatus) -> bool {
    matches!(
        status,
        LichessStatus::Aborted
            | LichessStatus::NoStart
            | LichessStatus::UnknownFinish
            | LichessStatus::Cheat
            | LichessStatus::VariantEnd
    )
}

/// Returns `Ok(())` when the export qualifies for `POST /attestations/void`.
pub fn check_void_eligible(game: &LichessGameExport) -> Result<(), (StatusCode, String)> {
    if matches!(
        game.status,
        LichessStatus::Created | LichessStatus::Started
    ) {
        return Err((
            StatusCode::BAD_REQUEST,
            format!(
                "game `{}` is ongoing (status `{:?}`); cannot attest void",
                game.id, game.status
            ),
        ));
    }

    if game.winner.is_some() {
        return Err((
            StatusCode::BAD_REQUEST,
            format!(
                "game `{}` has a decisive winner (`{:?}`); use POST /attestations/binary",
                game.id, game.winner
            ),
        ));
    }

    if matches!(
        game.status,
        LichessStatus::Draw | LichessStatus::Stalemate
    ) {
        return Err((
            StatusCode::BAD_REQUEST,
            format!(
                "game `{}` is a draw (status `{:?}`); use POST /attestations/draw",
                game.id, game.status
            ),
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::games::lichess::models::{LichessColor, LichessPlayer, LichessPlayers};

    fn game(status: LichessStatus, winner: Option<LichessColor>) -> LichessGameExport {
        LichessGameExport {
            id: "abcd1234".into(),
            status,
            winner,
            players: LichessPlayers {
                white: LichessPlayer { user: None },
                black: LichessPlayer { user: None },
            },
        }
    }

    #[test]
    fn void_statuses_match_spec() {
        for status in [
            LichessStatus::Aborted,
            LichessStatus::NoStart,
            LichessStatus::UnknownFinish,
            LichessStatus::Cheat,
            LichessStatus::VariantEnd,
        ] {
            assert!(is_void_lichess_status(status));
            assert!(check_void_eligible(&game(status, None)).is_ok());
        }
    }

    #[test]
    fn rejects_ongoing() {
        assert!(check_void_eligible(&game(LichessStatus::Created, None)).is_err());
        assert!(check_void_eligible(&game(LichessStatus::Started, None)).is_err());
    }

    #[test]
    fn rejects_decisive() {
        assert!(check_void_eligible(&game(LichessStatus::Mate, Some(LichessColor::White))).is_err());
    }

    #[test]
    fn rejects_draw() {
        assert!(check_void_eligible(&game(LichessStatus::Draw, None)).is_err());
        assert!(check_void_eligible(&game(LichessStatus::Stalemate, None)).is_err());
    }

    #[test]
    fn accepts_terminal_no_winner() {
        assert!(check_void_eligible(&game(LichessStatus::Resign, None)).is_ok());
        assert!(check_void_eligible(&game(LichessStatus::Timeout, None)).is_ok());
    }
}
