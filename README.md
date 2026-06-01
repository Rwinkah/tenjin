# Tenjin

Oracle service that verifies Lichess game outcomes and signs settlement attestations for **WagerFI** on-chain wagers.

## What it does

1. Accepts a wager id, Lichess game id, and white/black wallet addresses.
2. Fetches the finished game from Lichess.
3. Maps the winner to the correct wallet.
4. Returns a signed EIP-712 attestation for `LichessContract.lcSettle`.

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
| POST | `/attestations/binary` | Sign a 1v1 wager outcome |
| POST | `/attestations/competition` | Sign a ranked multi-player outcome |

### Binary attestation request

```json
{
  "wager_id": "0xabcd...",
  "game_id": "abc12345",
  "white_address": "0x...",
  "black_address": "0x..."
}
```

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
