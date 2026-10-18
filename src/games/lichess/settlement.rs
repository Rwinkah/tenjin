//! Lichess export classification for 1v1 wager attestations.
//!
//! Aligns with frontend `classifyLichessSettlement`: void when the game is
//! terminal without a white/black winner and is not a draw.

use axum::http::StatusCode;

use crate::games::lichess::models::{LichessGameExport, LichessStatus};

/// Settlement paths available for a multi-party tournament wager.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompetitionSettlementOption {
    Ranked,
    Void,
}

/// Result of evaluating which competition settlement paths are valid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompetitionPrepareSettle {
    pub ranked_eligible: bool,
    pub void_eligible: bool,
    pub recommended: Option<CompetitionSettlementOption>,
    pub reason: Option<String>,
}

/// Returns `Ok(())` when the export qualifies for `POST /attestations/void`.
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
    if matches!(game.status, LichessStatus::Created | LichessStatus::Started) {
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

    if matches!(game.status, LichessStatus::Draw | LichessStatus::Stalemate) {
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

/// Evaluates ranked vs void settlement for a multi-party wager from on-chain join counts.
pub fn prepare_competition_settle(
    participants_count: u64,
    min_joiners_to_settle: u64,
) -> CompetitionPrepareSettle {
    let ranked_eligible = participants_count >= min_joiners_to_settle;
    let void_eligible = participants_count > 0 && participants_count < min_joiners_to_settle;

    let recommended = if void_eligible {
        Some(CompetitionSettlementOption::Void)
    } else if ranked_eligible {
        Some(CompetitionSettlementOption::Ranked)
    } else {
        None
    };

    let reason = if void_eligible {
        Some(format!(
            "Only {participants_count} of {min_joiners_to_settle} required joiners — void returns each stake"
        ))
    } else if ranked_eligible {
        Some(format!(
            "{participants_count} joiners meet the minimum of {min_joiners_to_settle} for ranked payout"
        ))
    } else if participants_count == 0 {
        Some("No joiners yet — cancel before anyone joins or refund after expiry".into())
    } else {
        None
    };

    CompetitionPrepareSettle {
        ranked_eligible,
        void_eligible,
        recommended,
        reason,
    }
}

/// Returns `Ok(())` when a competition void attestation may be issued.
pub fn check_competition_void_eligible(
    participants_count: u64,
    min_joiners_to_settle: u64,
) -> Result<(), (StatusCode, String)> {
    let prep = prepare_competition_settle(participants_count, min_joiners_to_settle);
    if !prep.void_eligible {
        if prep.ranked_eligible {
            return Err((
                StatusCode::BAD_REQUEST,
                format!(
                    "{participants_count} joiners meets the minimum of {min_joiners_to_settle} — use ranked settlement"
                ),
            ));
        }
        return Err((
            StatusCode::BAD_REQUEST,
            "No joiners on-chain — cancel or wait for expiry instead of void settlement".into(),
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
            moves: None,
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
        assert!(
            check_void_eligible(&game(LichessStatus::Mate, Some(LichessColor::White))).is_err()
        );
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

    #[test]
    fn prepare_competition_settle_recommends_void_when_below_minimum() {
        let prep = prepare_competition_settle(2, 3);
        assert!(prep.void_eligible);
        assert!(!prep.ranked_eligible);
        assert_eq!(prep.recommended, Some(CompetitionSettlementOption::Void));
        assert!(check_competition_void_eligible(2, 3).is_ok());
    }

    #[test]
    fn prepare_competition_settle_recommends_ranked_when_enough_joiners() {
        let prep = prepare_competition_settle(3, 3);
        assert!(prep.ranked_eligible);
        assert!(!prep.void_eligible);
        assert_eq!(prep.recommended, Some(CompetitionSettlementOption::Ranked));
        assert!(check_competition_void_eligible(3, 3).is_err());
    }
}
