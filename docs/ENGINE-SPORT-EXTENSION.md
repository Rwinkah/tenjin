# Extending the tournament/match engine for a new sport

Tenjin’s engine is **sport-agnostic**. Core state machines live in `src/shared/engine/`.
Each game implements thin adapters — the same pattern as on-chain `AbstractWager`.

## What the core owns

| Module | Responsibility |
|--------|----------------|
| `leg.rs` | Single contest lifecycle (`PendingCreate` → terminal) |
| `series.rs` | Cage / multi-leg scoring (`first_to` / `best_of`) |
| `bracket.rs` / `tournament.rs` | Knockout seeding and advancement |
| `policy.rs` | Accept deadlines, draw replay, abandon → forfeit |
| `store.rs` / `postgres.rs` | Persistence + outbox events |
| `service.rs` + HTTP `/engine/{sport}/…` | Sport-parameterized API |

Anti-abandon rule (shared): **one escrow for the whole match**; unilateral miss of accept/start = **forfeit** (opponent wins), not void/refund. Only mutual failure voids.

## Traits to implement

```rust
// src/shared/engine/traits.rs
trait LegRuntime {
    fn sport_id(&self) -> SportId;
    fn describe_spawn(&self, ctx: &SpawnCtx, config: &Value) -> Value;
}

trait OutcomeClassifier {
    fn classify_value(&self, raw: &Value) -> LegOutcome;
}
```

Optional overrides:

- `SeriesRules` — custom scoring beyond first_to / best_of
- `BracketRules` — seeding / double-elim later

## Lichess reference adapter

| File | Role |
|------|------|
| `src/games/lichess/engine/classifier.rs` | Export status → `LegOutcome` |
| `src/games/lichess/engine/runtime.rs` | Spawn payload for Django executor |
| `src/games/lichess/engine/watcher.rs` | Poll Lichess export → `observe_leg` |

## Adding Football / Strava / FPL

1. Add `SportId::YourSport` in `types.rs` (and `as_str` / `parse`).
2. Create `src/games/<sport>/engine/` with:
   - `OutcomeClassifier` mapping your API’s terminal signals → `LegOutcome`
   - Optional watcher (`POST /engine/<sport>/watch-leg`)
3. Backend: sport executor that consumes `SpawnLeg` outbox events (create fixture / challenge / activity window).
4. Frontend: sport-specific create form writing intent JSON; reuse `CageMatchRoom` / `KnockoutBracketView` shells.
5. Contracts: keep settling once via existing BINARY / MULTI_PARTY paths when `ReadyToSettle` fires — no series opcodes required.

### Example mappings

| Sport | Leg = | Decisive | Forfeit |
|-------|-------|----------|---------|
| Lichess | Challenge/game | mate/resign/timeout | accept deadline miss |
| Football | Fixture | FT score | DNS / abandoned fixture (adapter policy) |
| Strava | Activity window | metric comparison | no activity by deadline |
| FPL | Gameweek / H2H | points | never started (existing forfeit path) |

## HTTP surface

- `POST /engine/{sport}/matches` — register single or series
- `GET /engine/{sport}/matches/{id}` / `…/by-wager/{wager_id}`
- `POST …/leg-created`, `…/observe`, `…/deadline`
- `POST /engine/{sport}/tournaments` + `…/seed` + `…/match-won`
- `GET /engine/events` + `POST /engine/events/{id}/ack` — outbox for executors

Do **not** put sport-specific branching inside `shared/engine` — keep classifiers in `games/<sport>/`.
