//! Poll Lichess game export and push observations into the shared engine.

use axum::http::StatusCode;
use reqwest::Client;
use serde::Deserialize;

use crate::games::lichess::engine::LichessOutcomeClassifier;
use crate::games::lichess::models::{LichessColor, LichessGameExport, LichessStatus};
use crate::shared::engine::service::EngineService;
use crate::shared::engine::types::{LegId, MatchId, SportId};
use crate::shared::engine::{LegOutcome, SeriesState, Side};

#[derive(Debug, Deserialize)]
pub struct WatchLegRequest {
    pub match_id: uuid::Uuid,
    pub leg_id: uuid::Uuid,
    pub external_ref: String,
    pub side_a_username: String,
    pub side_b_username: String,
}

fn ply_count(game: &LichessGameExport) -> usize {
    game.moves
        .as_deref()
        .map(|m| m.split_whitespace().filter(|p| !p.is_empty()).count())
        .unwrap_or(0)
}

fn side_for_color(
    classifier: &LichessOutcomeClassifier,
    game: &LichessGameExport,
    color: LichessColor,
) -> Option<Side> {
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
    let name = match color {
        LichessColor::White => white,
        LichessColor::Black => black,
    };
    if name == classifier.side_a_username {
        Some(Side::A)
    } else if name == classifier.side_b_username {
        Some(Side::B)
    } else {
        None
    }
}

/// Overdue start: white delinquent at 0 plies, black at 1 ply → forfeit that side.
fn overdue_start_forfeit(
    classifier: &LichessOutcomeClassifier,
    game: &LichessGameExport,
) -> Option<LegOutcome> {
    let still_pre_start = matches!(
        game.status,
        LichessStatus::Created | LichessStatus::Started | LichessStatus::NoStart
    );
    if !still_pre_start {
        return None;
    }
    let color = match ply_count(game) {
        0 => LichessColor::White,
        1 => LichessColor::Black,
        _ => return None,
    };
    let loser = side_for_color(classifier, game, color)?;
    Some(LegOutcome::Forfeit { loser })
}

/// Fetch export and apply to the engine. Returns updated match + events JSON.
pub async fn watch_and_observe(
    http: &Client,
    engine: &EngineService,
    req: WatchLegRequest,
) -> Result<(SeriesState, Vec<crate::shared::engine::EngineEvent>), (StatusCode, String)> {
    let url = format!("https://lichess.org/game/export/{}", req.external_ref);
    let response = http
        .get(&url)
        .query(&[
            ("moves", "true"),
            ("clocks", "false"),
            ("evals", "false"),
            ("opening", "false"),
        ])
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, e.to_string()))?;

    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Err((StatusCode::NOT_FOUND, "lichess game not found".into()));
    }
    if !response.status().is_success() {
        return Err((
            StatusCode::BAD_GATEWAY,
            format!("lichess export failed: {}", response.status()),
        ));
    }

    let game: LichessGameExport = response
        .json()
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, e.to_string()))?;

    let classifier =
        LichessOutcomeClassifier::new(&req.side_a_username, &req.side_b_username);
    let mut outcome = classifier.classify_export(&game);

    // Never started past accept deadline → forfeit delinquent color (backup to BE resign).
    if matches!(outcome, LegOutcome::Ongoing) {
        let state = engine
            .store()
            .get_match(SportId::Lichess, MatchId(req.match_id))
            .await
            .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
        let overdue = state
            .legs
            .iter()
            .find(|l| l.id == LegId(req.leg_id))
            .and_then(|l| l.accept_deadline)
            .map(|d| chrono::Utc::now() > d)
            .unwrap_or(false);
        if overdue {
            if let Some(forfeit) = overdue_start_forfeit(&classifier, &game) {
                outcome = forfeit;
            } else {
                return Ok((state, vec![]));
            }
        } else {
            return Ok((state, vec![]));
        }
    }

    engine
        .observe_leg(
            SportId::Lichess,
            MatchId(req.match_id),
            LegId(req.leg_id),
            outcome,
        )
        .await
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::games::lichess::models::{LichessPlayer, LichessPlayers, LichessUser};

    fn export(status: LichessStatus, moves: Option<&str>) -> LichessGameExport {
        LichessGameExport {
            id: "g1".into(),
            status,
            winner: None,
            players: LichessPlayers {
                white: LichessPlayer {
                    user: Some(LichessUser { id: "alice".into() }),
                },
                black: LichessPlayer {
                    user: Some(LichessUser { id: "bob".into() }),
                },
            },
            moves: moves.map(|m| m.into()),
        }
    }

    #[test]
    fn overdue_zero_plies_forfeits_white() {
        let c = LichessOutcomeClassifier::new("alice", "bob");
        let outcome = overdue_start_forfeit(&c, &export(LichessStatus::Started, None)).unwrap();
        assert_eq!(outcome, LegOutcome::Forfeit { loser: Side::A });
    }

    #[test]
    fn overdue_one_ply_forfeits_black() {
        let c = LichessOutcomeClassifier::new("alice", "bob");
        let outcome =
            overdue_start_forfeit(&c, &export(LichessStatus::Started, Some("e4"))).unwrap();
        assert_eq!(outcome, LegOutcome::Forfeit { loser: Side::B });
    }

    #[test]
    fn two_plies_not_start_forfeit() {
        let c = LichessOutcomeClassifier::new("alice", "bob");
        assert!(overdue_start_forfeit(&c, &export(LichessStatus::Started, Some("e4 e5"))).is_none());
    }
}
