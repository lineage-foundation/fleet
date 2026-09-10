//! Decoding a block header's committed PoW target (`bits`) into a readable form.
//!
//! The header stores the ASERT target as a Bitcoin-style `nBits` compact target
//! (`fleet_core::asert::CompactTarget`), carried verbatim in `BlockHeader::bits`.
//! A `bits` of `0` means "no committed target" (legacy/pre-activation). This
//! module turns a non-zero `bits` into a [`DifficultyTarget`] carrying both the
//! compact `nBits` word and the full expanded 256-bit target.

use fleet_core::asert::CompactTarget;
use serde::Serialize;
use serde_json::Value;
use utoipa::ToSchema;

/// A decoded, human-readable view of a block header's committed PoW target
/// (`bits`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct DifficultyTarget {
    /// The compact target in Bitcoin `nBits` form, e.g. `"0x1d00ffff"`.
    #[schema(example = "0x1d00ffff")]
    pub compact: String,
    /// The full 256-bit target the block hash must not exceed, as 64 lowercase
    /// hex characters (no `0x` prefix).
    #[schema(example = "00000000ffff0000000000000000000000000000000000000000000000000000")]
    pub target: String,
}

/// Decode a block header's `bits` field into a [`DifficultyTarget`].
///
/// Returns `None` when `bits == 0` (the "no committed target"/legacy sentinel)
/// or when `bits` is not a valid compact target.
pub fn decode_difficulty_target(bits: usize) -> Option<DifficultyTarget> {
    let compact = CompactTarget::from_bits(bits)?;
    // `CompactTarget`'s `Display` renders the `nBits` word as `0x`-prefixed,
    // zero-padded, 8-digit hex (e.g. `0x1d00ffff`).
    let compact_hex = compact.to_string();
    // The expanded target is a 256-bit unsigned integer; render it as 64
    // zero-padded lowercase hex digits.
    let target_hex = format!("{:0>64}", compact.expand_integer().to_string_radix(16));

    Some(DifficultyTarget {
        compact: compact_hex,
        target: target_hex,
    })
}

/// Decode the difficulty target from a stored block JSON value, reading the
/// `bits` at `value["block"]["header"]["bits"]` (the shape of
/// `fleet_core::interfaces::StoredSerializingBlock`).
///
/// Returns `None` when the path is absent, not an integer, or decodes to no
/// committed target.
pub fn difficulty_target_from_block_value(value: &Value) -> Option<DifficultyTarget> {
    let bits = value
        .get("block")?
        .get("header")?
        .get("bits")?
        .as_u64()?;
    decode_difficulty_target(bits as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_a_known_compact_target_to_compact_and_expanded_hex() {
        // 0x1d00ffff is Bitcoin's genesis/pow-limit compact target; its expansion
        // is the 256-bit value below (see `fleet_core::asert` expansion tests).
        let bits = 0x1d00ffff;
        let decoded = decode_difficulty_target(bits).expect("non-zero bits decode");

        assert_eq!(decoded.compact, "0x1d00ffff");
        assert_eq!(
            decoded.target,
            "00000000ffff0000000000000000000000000000000000000000000000000000"
        );
        assert_eq!(decoded.target.len(), 64);
    }

    #[test]
    fn decodes_a_second_known_compact_target() {
        // Bitcoin block 100,800: 0x1b0404cb.
        let decoded = decode_difficulty_target(0x1b0404cb).expect("non-zero bits decode");

        assert_eq!(decoded.compact, "0x1b0404cb");
        assert_eq!(
            decoded.target,
            "00000000000404cb000000000000000000000000000000000000000000000000"
        );
    }

    #[test]
    fn zero_bits_is_no_committed_target() {
        assert_eq!(decode_difficulty_target(0), None);
    }

    #[test]
    fn reads_bits_from_the_stored_block_json_shape() {
        let block = serde_json::json!({
            "block": {
                "header": {
                    "version": 1,
                    "bits": 0x1d00ffff,
                    "b_num": 7,
                },
                "transactions": [],
            }
        });

        let decoded = difficulty_target_from_block_value(&block).expect("bits present in json");
        assert_eq!(decoded.compact, "0x1d00ffff");
    }

    #[test]
    fn missing_or_zero_bits_in_json_yields_none() {
        assert_eq!(difficulty_target_from_block_value(&serde_json::json!({})), None);

        let legacy = serde_json::json!({ "block": { "header": { "bits": 0 } } });
        assert_eq!(difficulty_target_from_block_value(&legacy), None);
    }
}
