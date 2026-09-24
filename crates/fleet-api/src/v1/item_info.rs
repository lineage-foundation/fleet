//! Shared item genesis-facts DTO + pure extraction from a stored create `Transaction`.
//!
//! `item_info_from_tx` turns a stored genesis create-item transaction into the typed
//! response later `/v1/items/{genesis_hash}`-style handlers (storage, mempool/user
//! proxy) hand back to callers. It's read-side only: it doesn't touch `tx_is_valid` or
//! the on-chain item model, it just projects fields off an already-validated tx.

use prime::primitives::asset::Asset;
use prime::primitives::transaction::Transaction;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::error::ApiProblem;

/// Genesis facts for a single item, as recorded on its create transaction.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ItemInfoResponse {
    /// The item's genesis hash (the create transaction's hash).
    pub genesis_hash: String,
    /// Optional metadata attached at creation, if any.
    pub metadata: Option<String>,
    /// The total number of item units minted by the create transaction.
    pub total_amount: u64,
    /// Where and when the item was created.
    pub created: ItemCreated,
    /// The address the create output paid to, if any.
    pub creator_address: Option<String>,
}

/// Where a create transaction landed.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ItemCreated {
    /// The block number the create transaction was included in.
    pub block_num: u64,
    /// The create transaction's hash (same as `ItemInfoResponse::genesis_hash`).
    pub tx_hash: String,
}

/// Extract an `ItemInfoResponse` from a stored genesis create-item `Transaction`.
///
/// `genesis_hash` is the create transaction's hash (a caller-supplied lookup key, not
/// derived here); `block_num` is the block the transaction was included in. Returns
/// `Err` when `tx` has no `Asset::Item` output, i.e. it isn't an item genesis, or when
/// `tx` is an item transfer rather than the genesis create transaction.
pub fn item_info_from_tx(genesis_hash: &str, tx: &Transaction, block_num: u64) -> Result<ItemInfoResponse, ApiProblem> {
    if !tx.is_create_tx() {
        return Err(ApiProblem::not_found("transaction is not an item genesis"));
    }

    let item_out = tx
        .outputs
        .iter()
        .find(|out| out.value.is_item())
        .ok_or_else(|| ApiProblem::not_found("transaction is not an item genesis"))?;

    let Asset::Item(item) = &item_out.value else {
        unreachable!("just matched on Asset::is_item()");
    };

    Ok(ItemInfoResponse {
        genesis_hash: genesis_hash.to_owned(),
        metadata: item.metadata.clone(),
        total_amount: item.amount,
        created: ItemCreated {
            block_num,
            tx_hash: genesis_hash.to_owned(),
        },
        creator_address: item_out.script_public_key.clone(),
    })
}

#[cfg(test)]
mod tests {
    use prime::crypto::sign_ed25519 as sign;
    use prime::primitives::transaction::GenesisTxHashSpec;
    use prime::utils::transaction_utils::{construct_address, construct_item_create_tx};

    use super::item_info_from_tx;

    #[test]
    fn item_info_from_tx_extracts_fields() {
        let (public_key, secret_key) = sign::gen_keypair();
        let address = construct_address(&public_key);

        let tx = construct_item_create_tx(
            1,
            public_key,
            &secret_key,
            1000,
            GenesisTxHashSpec::Create,
            None,
            Some("m".to_owned()),
        );

        let genesis_hash = "genesis_tx_hash";
        let result = item_info_from_tx(genesis_hash, &tx, 7).expect("item genesis tx");

        assert_eq!(result.genesis_hash, genesis_hash);
        assert_eq!(result.metadata, Some("m".to_owned()));
        assert_eq!(result.total_amount, 1000);
        assert_eq!(result.creator_address, Some(address));
        assert_eq!(result.created.tx_hash, genesis_hash);
        assert_eq!(result.created.block_num, 7);
    }

    #[test]
    fn item_info_from_tx_no_metadata_is_null() {
        let (public_key, secret_key) = sign::gen_keypair();

        let tx = construct_item_create_tx(
            1,
            public_key,
            &secret_key,
            500,
            GenesisTxHashSpec::Create,
            None,
            None,
        );

        let result = item_info_from_tx("genesis_tx_hash", &tx, 1).expect("item genesis tx");

        assert_eq!(result.metadata, None);
    }

    #[test]
    fn item_info_from_tx_rejects_non_item_tx() {
        use prime::primitives::asset::{Asset, TokenAmount};
        use prime::primitives::transaction::{Transaction, TxOut};

        let tx = Transaction {
            inputs: vec![],
            outputs: vec![TxOut {
                value: Asset::Token(TokenAmount(42)),
                locktime: 0,
                script_public_key: Some("some_address".to_owned()),
            }],
            version: 1,
            fees: vec![],
            druid_info: None,
        };

        let result = item_info_from_tx("some_tx_hash", &tx, 1);

        assert!(result.is_err(), "expected a non-item tx to be rejected");
    }

    #[test]
    fn item_info_from_tx_rejects_non_create_item_tx() {
        use prime::primitives::asset::Asset;
        use prime::primitives::transaction::{OutPoint, Transaction, TxIn, TxOut};

        // Mirrors the transfer output shape built by `construct_rb_receive_payment_tx`:
        // an already-minted item being moved, referencing a previous output rather
        // than creating one.
        let tx = Transaction {
            inputs: vec![TxIn {
                previous_out: Some(OutPoint {
                    t_hash: "genesis_tx_hash".to_owned(),
                    n: 0,
                }),
                script_signature: Default::default(),
            }],
            outputs: vec![TxOut {
                value: Asset::item(1, Some("genesis_tx_hash".to_owned()), None),
                locktime: 0,
                script_public_key: Some("recipient_address".to_owned()),
            }],
            version: 1,
            fees: vec![],
            druid_info: None,
        };

        let result = item_info_from_tx("genesis_tx_hash", &tx, 1);

        assert!(result.is_err(), "expected a non-create item tx to be rejected");
    }
}
