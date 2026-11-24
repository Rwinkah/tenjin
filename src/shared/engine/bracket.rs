//! Single-elimination knockout bracket (sport-agnostic).

use serde::{Deserialize, Serialize};

use super::types::{MatchId, MatchKind, ParticipantId};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BracketSlot {
    pub match_id: MatchId,
    pub round: u32,
    pub index_in_round: u32,
    pub kind: MatchKind,
    pub side_a: Option<ParticipantId>,
    pub side_b: Option<ParticipantId>,
    pub winner: Option<ParticipantId>,
    /// Next match this winner feeds into (None for final).
    pub feeds_into: Option<MatchId>,
    pub feeds_side: Option<super::types::Side>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bracket {
    pub slots: Vec<BracketSlot>,
    pub entrants: Vec<ParticipantId>,
}

/// Seeds (1-based) in first-round bracket order for a power-of-two `size`: `[1, 4, 2, 3]` for 4,
/// `[1, 8, 4, 5, 2, 7, 3, 6]` for 8. Adjacent pairs are first-round matches; seeds `s` and `size + 1 - s`
/// meet, and the top two seeds can only meet in the final.
fn standard_seed_order(size: usize) -> Vec<usize> {
    let mut order = vec![1usize];
    let mut len = 1;
    while len < size {
        len *= 2;
        order = order.iter().flat_map(|&s| [s, len + 1 - s]).collect();
    }
    order
}

fn kind_for_round(round: u32, round_kinds: &[MatchKind]) -> MatchKind {
    round_kinds
        .get(round as usize)
        .copied()
        .or_else(|| round_kinds.last().copied())
        .unwrap_or(MatchKind::Single)
}

impl Bracket {
    /// Seed a power-of-two single-elim bracket. Pads with byes if needed.
    /// All rounds use the same `match_kind`.
    pub fn seed_single_elim(entrants: &[ParticipantId], match_kind: MatchKind) -> Self {
        Self::seed_single_elim_rounds(entrants, &[match_kind])
    }

    /// Seed with a match kind per round (index 0 = first round, last = final).
    /// Missing trailing rounds reuse the last entry; empty slice defaults to single.
    ///
    /// `entrants` is the seed order (index 0 = seed 1). First-round positions follow the standard
    /// bracket layout (1 v N, then the halves mirrored) and the missing seeds become byes, so the byes
    /// go to the top seeds and — because a bracket is always more than half full — every bye faces a
    /// real player. Appending byes at the end instead pairs byes with each other once more than one is
    /// needed, and those slots never resolve.
    pub fn seed_single_elim_rounds(entrants: &[ParticipantId], round_kinds: &[MatchKind]) -> Self {
        let n = entrants.len().next_power_of_two().max(2);
        let players: Vec<ParticipantId> = standard_seed_order(n)
            .into_iter()
            .map(|seed| match entrants.get(seed - 1) {
                Some(p) => p.clone(),
                None => ParticipantId::new(format!("BYE-{seed}")),
            })
            .collect();

        let rounds = (n as f64).log2() as u32;
        let mut slots: Vec<BracketSlot> = Vec::new();

        // Create all match slots first (round 0 = first round).
        let mut round_match_ids: Vec<Vec<MatchId>> = Vec::new();
        for round in 0..rounds {
            let count = n / 2usize.pow(round + 1);
            let mut ids = Vec::with_capacity(count);
            let kind = kind_for_round(round, round_kinds);
            for index_in_round in 0..count {
                let match_id = MatchId::new();
                ids.push(match_id);
                slots.push(BracketSlot {
                    match_id,
                    round,
                    index_in_round: index_in_round as u32,
                    kind,
                    side_a: None,
                    side_b: None,
                    winner: None,
                    feeds_into: None,
                    feeds_side: None,
                });
            }
            round_match_ids.push(ids);
        }

        // Wire feeds_into
        for round in 0..(rounds - 1) {
            for (i, &mid) in round_match_ids[round as usize].iter().enumerate() {
                let parent = round_match_ids[(round + 1) as usize][i / 2];
                let side = if i % 2 == 0 {
                    super::types::Side::A
                } else {
                    super::types::Side::B
                };
                if let Some(slot) = slots.iter_mut().find(|s| s.match_id == mid) {
                    slot.feeds_into = Some(parent);
                    slot.feeds_side = Some(side);
                }
            }
        }

        // Place entrants into round 0
        for (i, p) in players.iter().enumerate() {
            let match_index = i / 2;
            let side_a = i % 2 == 0;
            if let Some(slot) = slots
                .iter_mut()
                .find(|s| s.round == 0 && s.index_in_round == match_index as u32)
            {
                if side_a {
                    slot.side_a = Some(p.clone());
                } else {
                    slot.side_b = Some(p.clone());
                }
            }
        }

        // Auto-resolve byes
        let mut pending: Vec<(MatchId, ParticipantId)> = Vec::new();
        for slot in &slots {
            if slot.round != 0 {
                continue;
            }
            let a_bye = slot
                .side_a
                .as_ref()
                .map(|p| p.0.starts_with("BYE-"))
                .unwrap_or(false);
            let b_bye = slot
                .side_b
                .as_ref()
                .map(|p| p.0.starts_with("BYE-"))
                .unwrap_or(false);
            if a_bye ^ b_bye {
                let winner = if a_bye {
                    slot.side_b.clone().unwrap()
                } else {
                    slot.side_a.clone().unwrap()
                };
                pending.push((slot.match_id, winner));
            }
        }
        let mut bracket = Self {
            slots,
            entrants: entrants.to_vec(),
        };
        for (mid, winner) in pending {
            let _ = bracket.advance(mid, winner);
        }
        bracket
    }

    /// Record a match winner and place them in the next slot.
    /// Returns the next match id if both sides are now filled.
    pub fn advance(
        &mut self,
        from_match_id: MatchId,
        winner: ParticipantId,
    ) -> Result<Option<MatchId>, String> {
        let (feeds_into, feeds_side) = {
            let slot = self
                .slots
                .iter_mut()
                .find(|s| s.match_id == from_match_id)
                .ok_or_else(|| "unknown match".to_string())?;
            if slot.winner.is_some() {
                return Err("match already decided".into());
            }
            slot.winner = Some(winner.clone());
            (slot.feeds_into, slot.feeds_side)
        };

        let Some(next_id) = feeds_into else {
            return Ok(None);
        };
        let side = feeds_side.ok_or_else(|| "missing feeds_side".to_string())?;
        let next = self
            .slots
            .iter_mut()
            .find(|s| s.match_id == next_id)
            .ok_or_else(|| "missing parent match".to_string())?;
        match side {
            super::types::Side::A => next.side_a = Some(winner),
            super::types::Side::B => next.side_b = Some(winner),
        }
        if next.side_a.is_some() && next.side_b.is_some() {
            Ok(Some(next_id))
        } else {
            Ok(None)
        }
    }

    pub fn final_winner(&self) -> Option<&ParticipantId> {
        self.slots
            .iter()
            .filter(|s| s.feeds_into.is_none())
            .find_map(|s| s.winner.as_ref())
    }

    /// Placements: champion, then finalist. (MVP — extend later for 3rd place.)
    pub fn placements(&self) -> Vec<ParticipantId> {
        let mut out = Vec::new();
        if let Some(champ) = self.final_winner() {
            out.push(champ.clone());
            if let Some(final_slot) = self.slots.iter().find(|s| s.feeds_into.is_none()) {
                for side in [&final_slot.side_a, &final_slot.side_b] {
                    if let Some(p) = side {
                        if p != champ && !p.0.starts_with("BYE-") {
                            out.push(p.clone());
                        }
                    }
                }
            }
        }
        out
    }

    pub fn ready_matches(&self) -> Vec<&BracketSlot> {
        self.slots
            .iter()
            .filter(|s| {
                s.winner.is_none()
                    && s.side_a.is_some()
                    && s.side_b.is_some()
                    && !s.side_a.as_ref().unwrap().0.starts_with("BYE-")
                    && !s.side_b.as_ref().unwrap().0.starts_with("BYE-")
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn four_player_bracket_advances_to_final() {
        let a = ParticipantId::new("a");
        let b = ParticipantId::new("b");
        let c = ParticipantId::new("c");
        let d = ParticipantId::new("d");
        let mut br = Bracket::seed_single_elim(&[a.clone(), b.clone(), c.clone(), d.clone()], MatchKind::Single);
        assert_eq!(br.slots.len(), 3); // 2 semis + final
        let ready = br.ready_matches();
        assert_eq!(ready.len(), 2);
        let m0 = ready[0].match_id;
        let m1 = ready[1].match_id;
        let next = br.advance(m0, a.clone()).unwrap();
        assert!(next.is_none() || next.is_some());
        let _ = br.advance(m1, c.clone()).unwrap();
        let ready2 = br.ready_matches();
        assert_eq!(ready2.len(), 1);
        let final_id = ready2[0].match_id;
        let _ = br.advance(final_id, a.clone()).unwrap();
        assert_eq!(br.final_winner().unwrap().0, "a");
        assert_eq!(br.placements()[0].0, "a");
    }

    #[test]
    fn bye_auto_advances() {
        let a = ParticipantId::new("a");
        let b = ParticipantId::new("b");
        let c = ParticipantId::new("c");
        let br = Bracket::seed_single_elim(&[a, b, c], MatchKind::Series);
        // 4-slot bracket with one bye — at least one winner pre-filled via bye
        assert!(br.slots.iter().any(|s| s.winner.is_some() || s.round > 0 && s.side_a.is_some()));
    }

    fn is_bye(p: &Option<ParticipantId>) -> bool {
        p.as_ref().is_some_and(|p| p.0.starts_with("BYE-"))
    }

    fn seeded(count: usize) -> Vec<ParticipantId> {
        (1..=count).map(|i| ParticipantId::new(format!("s{i}"))).collect()
    }

    #[test]
    fn standard_seed_order_layout() {
        assert_eq!(standard_seed_order(2), vec![1, 2]);
        assert_eq!(standard_seed_order(4), vec![1, 4, 2, 3]);
        assert_eq!(standard_seed_order(8), vec![1, 8, 4, 5, 2, 7, 3, 6]);
    }

    #[test]
    fn no_bye_ever_meets_a_bye() {
        for count in 2..=64 {
            let br = Bracket::seed_single_elim(&seeded(count), MatchKind::Single);
            for slot in br.slots.iter().filter(|s| s.round == 0) {
                assert!(
                    !(is_bye(&slot.side_a) && is_bye(&slot.side_b)),
                    "{count} entrants: bye v bye in round 1"
                );
            }
        }
    }

    #[test]
    fn thirty_six_players_give_top_28_seeds_a_bye() {
        let br = Bracket::seed_single_elim(&seeded(36), MatchKind::Single);
        let round0: Vec<_> = br.slots.iter().filter(|s| s.round == 0).collect();
        assert_eq!(round0.len(), 32);

        // Byes are decided at seed time; the 4 real opening matches are seeds 29–36. Seeds 5–28 already
        // meet each other in 12 ready round-2 matches.
        let ready = br.ready_matches();
        assert_eq!(ready.iter().filter(|s| s.round == 1).count(), 12);
        let opening: Vec<_> = ready.iter().filter(|s| s.round == 0).collect();
        assert_eq!(opening.len(), 4);
        let mut playing: Vec<String> = opening
            .iter()
            .flat_map(|s| [s.side_a.clone().unwrap().0, s.side_b.clone().unwrap().0])
            .collect();
        playing.sort_by_key(|p| p[1..].parse::<u32>().unwrap());
        assert_eq!(playing, (29..=36).map(|i| format!("s{i}")).collect::<Vec<_>>());

        let advanced: Vec<_> = round0.iter().filter_map(|s| s.winner.as_ref()).collect();
        assert_eq!(advanced.len(), 28);
        assert!(advanced.iter().all(|p| p.0[1..].parse::<u32>().unwrap() <= 28));
    }

    #[test]
    fn odd_bracket_plays_through_to_a_champion() {
        // 5 players: 3 byes, then every round must become ready in turn.
        let mut br = Bracket::seed_single_elim(&seeded(5), MatchKind::Single);
        for _ in 0..3 {
            let ready = br.ready_matches();
            assert!(!ready.is_empty(), "bracket stalled");
            let decided: Vec<_> = ready.iter().map(|s| (s.match_id, s.side_a.clone().unwrap())).collect();
            for (id, winner) in decided {
                br.advance(id, winner).unwrap();
            }
            if br.final_winner().is_some() {
                break;
            }
        }
        assert_eq!(br.final_winner().unwrap().0, "s1");
    }

    #[test]
    fn per_round_kinds() {
        let players: Vec<_> = (0..8).map(|i| ParticipantId::new(format!("p{i}"))).collect();
        let br = Bracket::seed_single_elim_rounds(
            &players,
            &[MatchKind::Single, MatchKind::Series, MatchKind::Series],
        );
        assert!(br.slots.iter().filter(|s| s.round == 0).all(|s| s.kind == MatchKind::Single));
        assert!(br.slots.iter().filter(|s| s.round == 1).all(|s| s.kind == MatchKind::Series));
        assert!(br.slots.iter().filter(|s| s.round == 2).all(|s| s.kind == MatchKind::Series));
    }
}
