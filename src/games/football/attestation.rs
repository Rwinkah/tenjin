//! EIP-712 attestation builders matching `FootballOracle.sol`.

use alloy::primitives::{Address, B256, Bytes, U256};
use alloy::signers::Signer;
use alloy::signers::local::PrivateKeySigner;
use alloy::sol;
use alloy::sol_types::{SolStruct, eip712_domain};

sol! {
    #[derive(Debug)]
    struct BinaryOutcome {
        bytes32 wagerId;
        address winner;
        address loser;
        uint256 deadline;
    }

    #[derive(Debug)]
    struct DrawOutcome {
        bytes32 wagerId;
        uint256 deadline;
    }

    #[derive(Debug)]
    struct VoidOutcome {
        bytes32 wagerId;
        uint256 deadline;
    }

    #[derive(Debug)]
    struct CompetitionJoin {
        bytes32 wagerId;
        address joiner;
        uint256 deadline;
    }
}

fn football_domain(chain_id: u64, oracle: Address) -> alloy::sol_types::Eip712Domain {
    eip712_domain! {
        name: "FootballOracle",
        version: "1",
        chain_id: chain_id,
        verifying_contract: oracle,
    }
}

async fn sign_struct_hash(
    signer: &PrivateKeySigner,
    hash: B256,
) -> Result<[u8; 65], alloy::signers::Error> {
    let signature = signer.sign_hash(&hash).await?;
    Ok(signature.as_bytes())
}

fn encode_attestation_data(deadline: u64, signature: [u8; 65]) -> Bytes {
    crate::shared::attestation::encode_deadline_and_signature(deadline, &signature)
}

#[derive(Debug, Clone)]
pub struct OracleAttestation {
    pub oracle: Address,
    pub data: Bytes,
}

#[derive(Debug, Clone)]
pub struct JoinPermit {
    pub oracle: Address,
    pub deadline: u64,
    pub signature: Bytes,
}

pub async fn build_binary_attestation(
    signer: &PrivateKeySigner,
    oracle: Address,
    wager_id: B256,
    winner: Address,
    loser: Address,
    chain_id: u64,
    deadline: u64,
) -> Result<OracleAttestation, alloy::signers::Error> {
    let domain = football_domain(chain_id, oracle);
    let msg = BinaryOutcome {
        wagerId: wager_id,
        winner,
        loser,
        deadline: U256::from(deadline),
    };
    let hash = msg.eip712_signing_hash(&domain);
    let sig = sign_struct_hash(signer, hash).await?;
    Ok(OracleAttestation {
        oracle,
        data: encode_attestation_data(deadline, sig),
    })
}

pub async fn build_draw_attestation(
    signer: &PrivateKeySigner,
    oracle: Address,
    wager_id: B256,
    chain_id: u64,
    deadline: u64,
) -> Result<OracleAttestation, alloy::signers::Error> {
    let domain = football_domain(chain_id, oracle);
    let msg = DrawOutcome {
        wagerId: wager_id,
        deadline: U256::from(deadline),
    };
    let hash = msg.eip712_signing_hash(&domain);
    let sig = sign_struct_hash(signer, hash).await?;
    Ok(OracleAttestation {
        oracle,
        data: encode_attestation_data(deadline, sig),
    })
}

pub async fn build_void_attestation(
    signer: &PrivateKeySigner,
    oracle: Address,
    wager_id: B256,
    chain_id: u64,
    deadline: u64,
) -> Result<OracleAttestation, alloy::signers::Error> {
    let domain = football_domain(chain_id, oracle);
    let msg = VoidOutcome {
        wagerId: wager_id,
        deadline: U256::from(deadline),
    };
    let hash = msg.eip712_signing_hash(&domain);
    let sig = sign_struct_hash(signer, hash).await?;
    Ok(OracleAttestation {
        oracle,
        data: encode_attestation_data(deadline, sig),
    })
}

pub async fn build_join_permit(
    signer: &PrivateKeySigner,
    oracle: Address,
    wager_id: B256,
    joiner: Address,
    chain_id: u64,
    deadline: u64,
) -> Result<JoinPermit, alloy::signers::Error> {
    let domain = football_domain(chain_id, oracle);
    let msg = CompetitionJoin {
        wagerId: wager_id,
        joiner,
        deadline: U256::from(deadline),
    };
    let hash = msg.eip712_signing_hash(&domain);
    let sig = sign_struct_hash(signer, hash).await?;
    Ok(JoinPermit {
        oracle,
        deadline,
        signature: Bytes::from(sig.to_vec()),
    })
}
