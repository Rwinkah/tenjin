//! Classify FPL 1v1 and league-pot outcomes from official (data_checked) scores.

use alloy::primitives::Address;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryClass {
    CreatorWin,
    JoinerWin,
    Draw,
    Void,
    NotReady,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SideScore {
    pub points: Option<i32>,
    /// FPL `started_event`. None if unknown.
    pub started_event: Option<u32>,
}

pub fn classify_gameweek(event: u32, data_checked: bool, creator: SideScore, joiner: SideScore) -> BinaryClass {
    if !data_checked {
        return BinaryClass::NotReady;
    }
    match (creator.points, joiner.points) {
        (Some(a), Some(b)) => {
            if a > b {
                BinaryClass::CreatorWin
            } else if b > a {
                BinaryClass::JoinerWin
            } else {
                BinaryClass::Draw
            }
        }
        (Some(_), None) => forfeit_or_void(event, joiner.started_event, BinaryClass::CreatorWin),
        (None, Some(_)) => forfeit_or_void(event, creator.started_event, BinaryClass::JoinerWin),
        (None, None) => BinaryClass::Void,
    }
}

fn forfeit_or_void(event: u32, missing_started: Option<u32>, winner: BinaryClass) -> BinaryClass {
    match missing_started {
        Some(started) if started > event => winner,
        _ => BinaryClass::Void,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LeagueStanding {
    pub rank: Option<u32>,
    pub total: Option<i32>,
}

pub fn classify_league(
    data_checked: bool,
    settle_event: u32,
    creator: LeagueStanding,
    joiner: LeagueStanding,
    creator_started: Option<u32>,
    joiner_started: Option<u32>,
) -> BinaryClass {
    if !data_checked {
        return BinaryClass::NotReady;
    }
    match (creator.rank, joiner.rank) {
        (Some(a), Some(b)) => {
            if a < b {
                BinaryClass::CreatorWin
            } else if b < a {
                BinaryClass::JoinerWin
            } else {
                match (creator.total.unwrap_or(0), joiner.total.unwrap_or(0)) {
                    (ct, jt) if ct > jt => BinaryClass::CreatorWin,
                    (ct, jt) if jt > ct => BinaryClass::JoinerWin,
                    _ => BinaryClass::Draw,
                }
            }
        }
        (Some(_), None) => forfeit_or_void(settle_event, joiner_started, BinaryClass::CreatorWin),
        (None, Some(_)) => forfeit_or_void(settle_event, creator_started, BinaryClass::JoinerWin),
        (None, None) => BinaryClass::Void,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct H2hScore {
    pub creator_points: Option<i32>,
    pub joiner_points: Option<i32>,
    pub found: bool,
}

pub fn classify_h2h(
    event: u32,
    data_checked: bool,
    match_score: H2hScore,
    creator_started: Option<u32>,
    joiner_started: Option<u32>,
) -> BinaryClass {
    if !data_checked {
        return BinaryClass::NotReady;
    }
    if !match_score.found {
        return match (creator_started, joiner_started) {
            (Some(cs), Some(js)) if cs <= event && js > event => BinaryClass::CreatorWin,
            (Some(cs), Some(js)) if js <= event && cs > event => BinaryClass::JoinerWin,
            _ => BinaryClass::Void,
        };
    }
    match (match_score.creator_points, match_score.joiner_points) {
        (Some(a), Some(b)) if a > b => BinaryClass::CreatorWin,
        (Some(a), Some(b)) if b > a => BinaryClass::JoinerWin,
        (Some(_), Some(_)) => BinaryClass::Draw,
        (Some(_), None) => forfeit_or_void(event, joiner_started, BinaryClass::CreatorWin),
        (None, Some(_)) => forfeit_or_void(event, creator_started, BinaryClass::JoinerWin),
        (None, None) => BinaryClass::Void,
    }
}

#[derive(Debug, Clone)]
pub struct RosterStanding {
    pub wallet: Address,
    pub entry_id: u64,
    pub rank: Option<u32>,
    pub total: Option<i32>,
}

/// Rank-1 among joiners+creator by FPL overall rank, then total points. Empty if none scored.
pub fn competition_placements(roster: &[RosterStanding]) -> Vec<(u64, Address)> {
    let mut scored: Vec<&RosterStanding> = roster
        .iter()
        .filter(|row| row.rank.is_some() || row.total.is_some())
        .collect();
    scored.sort_by(|a, b| {
        let ra = a.rank.unwrap_or(u32::MAX);
        let rb = b.rank.unwrap_or(u32::MAX);
        ra.cmp(&rb)
            .then_with(|| b.total.unwrap_or(0).cmp(&a.total.unwrap_or(0)))
    });
    scored
        .into_iter()
        .enumerate()
        .map(|(idx, row)| ((idx as u64) + 1, row.wallet))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::address;

    fn scored(points: i32) -> SideScore {
        SideScore {
            points: Some(points),
            started_event: Some(1),
        }
    }

    fn missing(started: Option<u32>) -> SideScore {
        SideScore {
            points: None,
            started_event: started,
        }
    }

    #[test]
    fn gw_higher_points_wins() {
        assert_eq!(
            classify_gameweek(1, true, scored(70), scored(64)),
            BinaryClass::CreatorWin
        );
        assert_eq!(
            classify_gameweek(1, true, scored(50), scored(80)),
            BinaryClass::JoinerWin
        );
    }

    #[test]
    fn gw_equal_is_draw() {
        assert_eq!(
            classify_gameweek(1, true, scored(66), scored(66)),
            BinaryClass::Draw
        );
    }

    #[test]
    fn gw_not_ready_until_data_checked() {
        assert_eq!(
            classify_gameweek(1, false, scored(90), scored(10)),
            BinaryClass::NotReady
        );
    }

    #[test]
    fn gw_void_when_both_missing() {
        assert_eq!(
            classify_gameweek(1, true, missing(Some(1)), missing(Some(1))),
            BinaryClass::Void
        );
    }

    #[test]
    fn gw_forfeit_when_other_never_started() {
        assert_eq!(
            classify_gameweek(1, true, scored(44), missing(Some(3))),
            BinaryClass::CreatorWin
        );
    }

    #[test]
    fn gw_void_when_missing_but_had_started() {
        assert_eq!(
            classify_gameweek(2, true, scored(44), missing(Some(1))),
            BinaryClass::Void
        );
    }

    #[test]
    fn league_better_rank_wins() {
        assert_eq!(
            classify_league(
                true,
                38,
                LeagueStanding {
                    rank: Some(2),
                    total: Some(2000)
                },
                LeagueStanding {
                    rank: Some(8),
                    total: Some(1800)
                },
                Some(1),
                Some(1),
            ),
            BinaryClass::CreatorWin
        );
    }

    #[test]
    fn league_tiebreak_total_points() {
        assert_eq!(
            classify_league(
                true,
                38,
                LeagueStanding {
                    rank: Some(4),
                    total: Some(2100)
                },
                LeagueStanding {
                    rank: Some(4),
                    total: Some(2090)
                },
                Some(1),
                Some(1),
            ),
            BinaryClass::CreatorWin
        );
    }

    #[test]
    fn h2h_draw() {
        assert_eq!(
            classify_h2h(
                1,
                true,
                H2hScore {
                    creator_points: Some(50),
                    joiner_points: Some(50),
                    found: true,
                },
                Some(1),
                Some(1),
            ),
            BinaryClass::Draw
        );
    }

    #[test]
    fn h2h_missing_pairing_is_void() {
        assert_eq!(
            classify_h2h(
                1,
                true,
                H2hScore {
                    creator_points: None,
                    joiner_points: None,
                    found: false,
                },
                Some(1),
                Some(1),
            ),
            BinaryClass::Void
        );
    }

    #[test]
    fn competition_rank_one_takes_pot() {
        let a = address!("0x0000000000000000000000000000000000000001");
        let b = address!("0x0000000000000000000000000000000000000002");
        let c = address!("0x0000000000000000000000000000000000000003");
        let placements = competition_placements(&[
            RosterStanding {
                wallet: a,
                entry_id: 1,
                rank: Some(12),
                total: Some(100),
            },
            RosterStanding {
                wallet: b,
                entry_id: 2,
                rank: Some(3),
                total: Some(180),
            },
            RosterStanding {
                wallet: c,
                entry_id: 3,
                rank: Some(7),
                total: Some(150),
            },
        ]);
        assert_eq!(placements[0], (1, b));
        assert_eq!(placements[1], (2, c));
        assert_eq!(placements[2], (3, a));
    }
}
