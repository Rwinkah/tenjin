//! Poll Lichess game export and push observations into the shared engine.

use axum::http::StatusCode;
use reqwest::Client;
use serde::Deserialize;

use crate::games::lichess::engine::LichessOutcomeClassifier;
use crate::games::lichess::models::LichessGameExport;
use crate::shared::engine::service::EngineService;
use crate::shared::engine::types::{LegId, MatchId, SportId};
use crate::shared::engine::{LegOutcome, SeriesState};

#[derive(Debug, Deserialize)]
pub struct WatchLegRequest {
    pub match_id: uuid::Uuid,
    pub leg_id: uuid::Uuid,
    pub external_ref: String,
    pub side_a_username: String,
    pub side_b_username: String,
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
            ("moves", "false"),
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

    // Never started / still in lobby past accept deadline → mutual no-show void.
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
        let still_lobby = matches!(
            game.status,
            crate::games::lichess::models::LichessStatus::Created
                | crate::games::lichess::models::LichessStatus::NoStart
        );
        if overdue && still_lobby {
            outcome = LegOutcome::MutualVoid;
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
