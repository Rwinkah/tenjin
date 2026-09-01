//! Shared `OracleAttestation.data` encoding.
//!
//! On-chain oracles decode `abi.encode(uint256 deadline, bytes signature)` and
//! require the dynamic `bytes` offset to be exactly 64. Alloy's
//! `(U256, Vec<u8>).abi_encode()` wraps that tuple as a *single dynamic value*
//! (leading offset `0x20`), which the decoder rejects as malformed.

use alloy::primitives::{Bytes, U256};

/// Solidity `abi.encode(uint256 deadline, bytes signature)`.
pub fn encode_deadline_and_signature(deadline: u64, signature: &[u8]) -> Bytes {
    let mut out = Vec::with_capacity(96 + signature.len().div_ceil(32) * 32);
    out.extend_from_slice(&U256::from(deadline).to_be_bytes::<32>());
    out.extend_from_slice(&U256::from(64u64).to_be_bytes::<32>());
    out.extend_from_slice(&U256::from(signature.len()).to_be_bytes::<32>());
    out.extend_from_slice(signature);
    let pad = (32 - signature.len() % 32) % 32;
    out.extend(std::iter::repeat_n(0u8, pad));
    out.into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::sol_types::SolValue;

    #[test]
    fn layout_matches_solidity_abi_encode_params() {
        let sig = [0xABu8; 65];
        let data = encode_deadline_and_signature(1_700_000_000, &sig);
        assert!(data.len() >= 129);
        let offset = U256::from_be_bytes::<32>(data[32..64].try_into().unwrap());
        assert_eq!(offset, U256::from(64));
        let len = U256::from_be_bytes::<32>(data[64..96].try_into().unwrap());
        assert_eq!(len, U256::from(65));
        assert_eq!(&data[96..161], &sig);
    }

    #[test]
    fn tuple_abi_encode_is_not_the_oracle_layout() {
        let sig = vec![0xABu8; 65];
        let wrapped = (U256::from(1_700_000_000u64), sig).abi_encode();
        let first = U256::from_be_bytes::<32>(wrapped[0..32].try_into().unwrap());
        // Dynamic-tuple wrapping: first word is an offset, not the deadline.
        assert_eq!(first, U256::from(32));
    }
}
