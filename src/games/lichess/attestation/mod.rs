//! Builders for `OracleAttestation` payloads consumed by
//! `LichessContract.lcSettle` / `lcSettleCompetition`.
//!
//! Matches `LichessOracle.sol`:
//! - EIP-712 domain: `LichessOracle` / `1` / chainId / verifyingContract
//! - Binary struct: `BinaryOutcome(bytes32 wagerId,address winner,address loser,uint256 deadline)`
//! - Competition struct: `CompetitionOutcome(bytes32 wagerId,bytes32 placementsHash,uint256 deadline)`
//! - `OracleAttestation.data` is `abi.encode(uint256 deadline, bytes signature)`

use std::time::{SystemTime, UNIX_EPOCH};

use alloy::primitives::{keccak256, Address, B256, Bytes, U256};
use alloy::signers::local::PrivateKeySigner;
use alloy::signers::Signer;
use alloy::sol;
use alloy::sol_types::{eip712_domain, SolStruct, SolValue};

const ATTESTATION_DEADLINE_SECS: u64 = 7 * 24 * 3600;

sol! {
    struct BinaryOutcome {
        bytes32 wagerId;
        address winner;
        address loser;
        uint256 deadline;
    }

    struct CompetitionOutcome {
        bytes32 wagerId;
        bytes32 placementsHash;
        uint256 deadline;
    }

    /// Matches `IWagerOracle.RankedPlacement`.
    struct RankedPlacement {
        uint256 rank;
        address player;
    }
}

/// One row of the `OracleAttestation[]` argument the wager contract expects.
#[derive(Debug, Clone)]
pub struct OracleAttestation {
    pub oracle: Address,
    /// `abi.encode(uint256 deadline, bytes signature)`.
    pub data: Bytes,
}

fn attestation_deadline() -> U256 {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before UNIX epoch")
        .as_secs();
    U256::from(now + ATTESTATION_DEADLINE_SECS)
}

fn lichess_oracle_domain(chain_id: u64, oracle: Address) -> alloy::sol_types::Eip712Domain {
    eip712_domain! {
        name: "LichessOracle",
        version: "1",
        chain_id: chain_id,
        verifying_contract: oracle,
    }
}

fn encode_attestation_data(deadline: U256, signature: alloy::primitives::Signature) -> Bytes {
    let sig = signature.as_bytes();
    let mut out = Vec::with_capacity(96 + sig.len().div_ceil(32) * 32);

    // Solidity: abi.encode(uint256 deadline, bytes signature)
    out.extend_from_slice(&deadline.to_be_bytes::<32>());
    out.extend_from_slice(&U256::from(64).to_be_bytes::<32>());
    out.extend_from_slice(&U256::from(sig.len()).to_be_bytes::<32>());
    out.extend_from_slice(&sig);
    let pad = (32 - sig.len() % 32) % 32;
    out.extend(std::iter::repeat_n(0u8, pad));

    out.into()
}

/// Builds the on-chain attestation for a binary (1v1) settlement.
pub async fn build_binary_attestation(
    signer: &PrivateKeySigner,
    oracle: Address,
    wager_id: B256,
    winner: Address,
    loser: Address,
    chain_id: u64,
) -> Result<OracleAttestation, alloy::signers::Error> {
    let deadline = attestation_deadline();
    let domain = lichess_oracle_domain(chain_id, oracle);
    let message = BinaryOutcome {
        wagerId: wager_id,
        winner,
        loser,
        deadline,
    };

    let signing_hash = message.eip712_signing_hash(&domain);
    let signature = signer.sign_hash(&signing_hash).await?;
    let data = encode_attestation_data(deadline, signature);

    Ok(OracleAttestation { oracle, data })
}

/// `keccak256(abi.encode(RankedPlacement[]))` — must match `LichessOracle.hashPlacements`.
pub fn hash_placements(placements: &[RankedPlacement]) -> B256 {
    keccak256(placements.abi_encode())
}

/// Builds the on-chain attestation for a competition (ranked) settlement.
pub async fn build_competition_attestation(
    signer: &PrivateKeySigner,
    oracle: Address,
    wager_id: B256,
    placements: &[RankedPlacement],
    chain_id: u64,
) -> Result<OracleAttestation, alloy::signers::Error> {
    let placements_hash = hash_placements(placements);

    let deadline = attestation_deadline();
    let domain = lichess_oracle_domain(chain_id, oracle);
    let message = CompetitionOutcome {
        wagerId: wager_id,
        placementsHash: placements_hash,
        deadline,
    };

    let signing_hash = message.eip712_signing_hash(&domain);
    let signature = signer.sign_hash(&signing_hash).await?;
    let data = encode_attestation_data(deadline, signature);

    Ok(OracleAttestation { oracle, data })
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::address;

    fn test_signer(chain_id: u64) -> PrivateKeySigner {
        PrivateKeySigner::random().with_chain_id(Some(chain_id))
    }

    #[tokio::test]
    async fn binary_attestation_data_is_deadline_and_signature() {
        let signer = test_signer(31337);
        let oracle = address!("0xe7f1725E7734CE288F8367e1Bb143E90bb3F0512");
        let wager_id = B256::from([0xab; 32]);
        let winner = address!("1111111111111111111111111111111111111111");
        let loser = address!("2222222222222222222222222222222222222222");

        let attestation = build_binary_attestation(&signer, oracle, wager_id, winner, loser, 31337)
            .await
            .unwrap();

        assert_eq!(attestation.oracle, oracle);
        assert!(attestation.data.len() >= 96);
        let deadline_word = U256::from_be_slice(&attestation.data[0..32]);
        assert!(
            deadline_word > U256::from(1_000_000_000u64),
            "attestation data should start with deadline, got 0x{deadline_word:x}"
        );
        assert_ne!(
            deadline_word,
            U256::from(32),
            "attestation data looks double-wrapped"
        );
    }

    #[test]
    fn hash_placements_matches_solidity_encoding() {
        let alice = address!("0x90F79bf6EB2c4f870365E785982E1f101E93b906");
        let charlie = address!("0x9965507D1a55bcC2695C58ba16FB37d819B0A4dc");
        let placements = vec![
            RankedPlacement {
                rank: U256::from(1),
                player: alice,
            },
            RankedPlacement {
                rank: U256::from(5),
                player: charlie,
            },
        ];

        let hash = hash_placements(&placements);
        assert_eq!(
            hash,
            B256::from([
                0xcd, 0xf0, 0x69, 0x9d, 0x04, 0xbc, 0xdf, 0xe0, 0x1d, 0x49, 0x4f, 0x04, 0x52,
                0xc7, 0x46, 0xe4, 0x40, 0x63, 0xb7, 0x4b, 0x3e, 0x08, 0xf2, 0xda, 0xf7, 0x96,
                0x23, 0x8a, 0x1d, 0x50, 0xc4, 0x1d,
            ])
        );
    }

    #[tokio::test]
    async fn competition_attestation_data_is_deadline_and_signature() {
        let signer = test_signer(31337);
        let oracle = address!("0xe7f1725E7734CE288F8367e1Bb143E90bb3F0512");
        let wager_id = B256::from([0xcd; 32]);
        let placements = vec![RankedPlacement {
            rank: U256::from(1),
            player: address!("0x90F79bf6EB2c4f870365E785982E1f101E93b906"),
        }];

        let attestation =
            build_competition_attestation(&signer, oracle, wager_id, &placements, 31337)
                .await
                .unwrap();

        assert_eq!(attestation.oracle, oracle);
        assert!(attestation.data.len() >= 96);
    }
}
