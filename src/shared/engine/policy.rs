//! Default deadline / draw / abandon policies (sports may override).

use chrono::Duration;
use serde::{Deserialize, Serialize};

use super::types::SeriesRule;

/// Accept/start window bounds for cage / series legs (seconds).
pub const MIN_ACCEPT_DEADLINE_SECS: u64 = 3 * 60;
pub const MAX_ACCEPT_DEADLINE_SECS: u64 = 15 * 60;

/// How long a participant has to start/accept a spawned leg.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeadlinePolicy {
    /// Seconds after SpawnLeg before AcceptTimedOut.
    pub accept_deadline_secs: u64,
}

impl Default for DeadlinePolicy {
    fn default() -> Self {
        Self {
            accept_deadline_secs: MIN_ACCEPT_DEADLINE_SECS,
        }
    }
}

impl DeadlinePolicy {
    pub fn clamped(self) -> Self {
        Self {
            accept_deadline_secs: self
                .accept_deadline_secs
                .clamp(MIN_ACCEPT_DEADLINE_SECS, MAX_ACCEPT_DEADLINE_SECS),
        }
    }

    pub fn from_secs(secs: u64) -> Self {
        Self {
            accept_deadline_secs: secs,
        }
        .clamped()
    }
}

/// What happens when a leg draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DrawPolicy {
    /// Replay the same leg config (default for first_to / best_of).
    ReplayLeg,
    /// Count as no win and advance to next scheduled leg config if any.
    SkipToNextConfig,
}

impl Default for DrawPolicy {
    fn default() -> Self {
        Self::ReplayLeg
    }
}

/// Scope of a unilateral abandon / accept timeout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum UnilateralAbandonScope {
    /// Abandoning any leg forfeits the entire cage / series match.
    #[default]
    EntireMatch,
    /// Abandoning only costs that leg; the series continues if not decided.
    CurrentLeg,
}

/// Escrow / abandon economics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AbandonPolicy {
    /// Unilateral miss of accept/start → forfeit (not void).
    pub unilateral_is_forfeit: bool,
    /// Both sides miss → mutual void (only refund path).
    pub bilateral_is_void: bool,
    /// Whether a unilateral forfeit ends the whole series or only the current leg.
    #[serde(default)]
    pub unilateral_scope: UnilateralAbandonScope,
}

impl Default for AbandonPolicy {
    fn default() -> Self {
        Self {
            unilateral_is_forfeit: true,
            bilateral_is_void: true,
            unilateral_scope: UnilateralAbandonScope::EntireMatch,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeriesConfig {
    pub rule: SeriesRule,
    /// Target wins for FirstTo, or N for BestOf (must be odd for BestOf).
    pub n: u32,
    pub draw: DrawPolicy,
    pub deadlines: DeadlinePolicy,
    pub abandon: AbandonPolicy,
}

impl SeriesConfig {
    pub fn first_to(n: u32) -> Self {
        Self {
            rule: SeriesRule::FirstTo,
            n: n.max(1),
            draw: DrawPolicy::default(),
            deadlines: DeadlinePolicy::default(),
            abandon: AbandonPolicy::default(),
        }
    }

    pub fn best_of(n: u32) -> Self {
        let n = if n % 2 == 0 { n + 1 } else { n };
        Self {
            rule: SeriesRule::BestOf,
            n: n.max(1),
            draw: DrawPolicy::default(),
            deadlines: DeadlinePolicy::default(),
            abandon: AbandonPolicy::default(),
        }
    }

    pub fn wins_needed(&self) -> u32 {
        match self.rule {
            SeriesRule::FirstTo => self.n,
            SeriesRule::BestOf => (self.n / 2) + 1,
        }
    }

    pub fn accept_deadline_duration(&self) -> Duration {
        Duration::seconds(self.deadlines.clamped().accept_deadline_secs as i64)
    }

    pub fn with_clamped_deadlines(mut self) -> Self {
        self.deadlines = self.deadlines.clamped();
        self
    }
}
