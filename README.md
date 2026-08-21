# Tenjin

Oracle service that verifies off-chain results and signs settlement attestations for **WagerFI** on-chain wagers.

## What it does

1. Accepts a wager id plus game-specific inputs (Lichess game, football fixture, or Strava outcome).
2. Verifies the result from the source of truth where possible (Lichess API, football-data.org).
3. Maps the winner to the correct wallet (or signs a Django-verified Strava outcome).
4. Returns a signed EIP-712 attestation for the matching oracle contract.

## Stack

- **Rust** (edition 2024)
- **[Axum](https://github.com/tokio-rs/axum)** — HTTP API
- **[Alloy](https://github.com/alloy-rs/alloy)** — EIP-712 signing

## Configuration

Copy `.env.example` to `.env` and set:

| Variable | Description |
|----------|-------------|
| `PRIVATE_KEY` | Key for an address registered as `LichessOracle` attester |
| `CHAIN_ID` | Chain id where the oracle contract lives (`31337` for local Hardhat) |
| `LICHESS_ORACLE_ADDRESS` | Deployed `LichessOracle` contract address (same as wager-backend) |
| `FOOTBALL_ORACLE_ADDRESS` | Deployed `FootballOracle` (optional) |
| `STRAVA_ORACLE_ADDRESS` | Deployed `StravaOracle` (optional; required for `/strava/*`) |
| `PORT` | HTTP port (default `3001`) |

For local dev with wagerv2:

```bash
# wagerv2 repo — terminal 1
yarn chain && yarn deploy

# tenjin repo — terminal 2
cp .env.example .env
cargo run
```

Point the frontend at Tenjin:

```
NEXT_PUBLIC_TENJIN_URI=http://127.0.0.1:3001
```

## API

| Method | Path | Description |
|--------|------|-------------|
| GET | `/` | Service health (signer + chain id) |
| GET | `/health` | Lichess module health |
| POST | `/attestations/binary` | Sign a 1v1 decisive outcome |
| POST | `/attestations/draw` | Sign a 1v1 draw (refund both stakes) |
| POST | `/attestations/void` | Sign a 1v1 void (no valid win/loss/draw) |
| POST | `/attestations/competition` | Sign a ranked multi-player outcome |

| POST | `/attestations/join` | Sign a join permit (`kind`: `binary` \| `competition`) for `lcJoinWithPermit` |
| POST | `/attestations/competition-join` | Deprecated alias for `kind: competition` |
| GET | `/strava/health` | Strava module health |
| POST | `/strava/attestations/join` | Sign a Strava join permit |
| POST | `/strava/attestations/binary` | Sign a Strava 1v1 outcome |
| POST | `/strava/attestations/draw` | Sign a Strava draw |
| POST | `/strava/attestations/void` | Sign a Strava void |

### Join attestation request

```json
{
  "kind": "binary",
  "wager_id": "0xabcd...",
  "joiner": "0x...",
  "game_id": "{\"v\":1,\"open\":false,\"opponent\":\"alice\",...}",
  "joiner_lichess_username": "alice",
  "challenge_id": "AbCdEfGh"
}
```

Competition joins omit `game_id`, `joiner_lichess_username`, and `challenge_id`:

```json
{
  "kind": "competition",
  "wager_id": "0xabcd...",
  "joiner": "0x..."
}
```

Response: `{ "oracle", "deadline", "signature" }`.

Requires `ETH_RPC_URL` and `LICHESS_CONTRACT_ADDRESS` for on-chain join validation.

### Binary attestation request

```json
{
  "wager_id": "0xabcd...",
  "game_id": "abc12345",
  "white_address": "0x...",
  "black_address": "0x..."
}
```

### Draw / void attestation request

Same shape for draw and void (oracle fetches the Lichess game and classifies the result):

```json
{
  "wager_id": "0xabcd...",
  "game_id": "abc12345"
}
```

Response: `{ "oracle", "data", "game_id", "raw_status" }`. Pass into `lcSettleDraw` or `lcSettleVoid` respectively.

### Competition attestation request

```json
{
  "wager_id": "0xabcd...",
  "placements": [
    { "rank": 1, "player": "0xAlice..." },
    { "rank": 2, "player": "0xBob..." }
  ]
}
```

Ranks are **1-based** and may be sparse (e.g. ranks 1 and 5). The array **order must match** exactly what you pass to `lcSettleCompetition` — reordering entries changes the hash and invalidates the attestation.

Response includes `oracle`, `data`, and echo of `placements`.

### Strava attestations

Strava OAuth tokens live in wager-backend. Django fetches activities, applies the metric rules, then asks Tenjin to sign against `StravaOracle`.

Join:

```json
{
  "wager_id": "0xabcd...",
  "joiner": "0x...",
  "game_id": "{\"v\":1,\"open\":true,\"metric\":\"distance\",\"compare\":\"max\"}",
  "chain_id": 77777
}
```

Binary (winner/loser already decided by Django):

```json
{
  "wager_id": "0xabcd...",
  "winner": "0x...",
  "loser": "0x...",
  "creator_activity_id": "123",
  "joiner_activity_id": "456",
  "chain_id": 77777
}
```

Join response: `{ "oracle", "deadline", "signature" }`. Settle response: `{ "oracle", "data", ... }` where `data` is `abi.encode(deadline, signature)`.

## Run

```bash
cargo run
```

## Test

```bash
cargo test
```

## License

Add a `LICENSE` file when you choose one for this repository.
