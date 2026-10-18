//! Lichess [`LegRuntime`] — describes spawn payloads for backend executors.

use serde_json::{Value, json};

use crate::shared::engine::traits::{LegRuntime, SpawnCtx};
use crate::shared::engine::types::SportId;

#[derive(Debug, Default)]
pub struct LichessLegRuntime;

impl LegRuntime for LichessLegRuntime {
    fn sport_id(&self) -> SportId {
        SportId::Lichess
    }

    fn describe_spawn(&self, ctx: &SpawnCtx, config: &Value) -> Value {
        json!({
            "sport": "lichess",
            "action": "create_challenge",
            "wager_id": ctx.wager_id,
            "match_id": ctx.match_id,
            "side_a": ctx.side_a,
            "side_b": ctx.side_b,
            "config": config,
        })
    }
}
