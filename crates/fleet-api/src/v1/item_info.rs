//! Shared item genesis-facts DTO + pure extraction from a stored create `Transaction`,
//! plus the storage node's `GET /v1/items/{genesis_hash}` handler built on it.
//!
//! `item_info_from_tx` turns a stored genesis create-item transaction into the typed
//! response `/v1/items/{genesis_hash}`-style handlers (storage, mempool/user proxy)
//! hand back to callers. It's read-side only: it doesn't touch `tx_is_valid` or the
//! on-chain item model, it just projects fields off an already-validated tx.

use axum::extract::{Path, State};
use axum::Json;
use bincode::deserialize;
use fleet_core::db_utils::get_stored_value_from_db;
use fleet_core::interfaces::BlockchainItemMeta;
use prime::primitives::asset::Asset;
use prime::primitives::transaction::Transaction;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::error::ApiProblem;
use crate::state::ApiState;

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

/// Get genesis facts for a single item, by its create transaction's hash.
///
/// The lookup is a direct DB read keyed on `genesis_hash` (the same raw-key read
/// `get_blockchain_entry` uses), so a later transfer of the item has no bearing on
/// this: the entry stored under `genesis_hash` is always the create transaction.
#[utoipa::path(
    get,
    path = "/v1/items/{genesis_hash}",
    tag = "items",
    params(("genesis_hash" = String, Path, description = "The item's genesis hash (its create transaction's hash)")),
    responses(
        (status = 200, description = "The item's genesis facts", body = ItemInfoResponse),
        (status = 404, description = "No item genesis stored at this hash", body = ApiProblem, content_type = "application/problem+json"),
    ),
)]
pub async fn get_item_info_storage(
    State(state): State<ApiState>,
    Path(genesis_hash): Path<String>,
) -> Result<Json<ItemInfoResponse>, ApiProblem> {
    let db = state
        .db
        .clone()
        .ok_or_else(|| ApiProblem::internal("this node does not expose a blockchain database"))?;

    let item = get_stored_value_from_db(db, &genesis_hash)
        .ok_or_else(|| ApiProblem::not_found("no item stored at this genesis hash"))?;

    let BlockchainItemMeta::Tx { block_num, .. } = item.item_meta else {
        return Err(ApiProblem::not_found("no item stored at this genesis hash"));
    };

    // The bincode-encoded `data`, not `data_json`: some tx fields (e.g. signatures)
    // deserialize from a borrowed byte slice, which `data_json`'s JSON array
    // encoding of those bytes can't satisfy (`item_to_entry_response`'s untyped
    // `serde_json::Value` decode of `data_json` sidesteps this; a typed decode can't).
    let tx: Transaction = deserialize(&item.data).map_err(|err| ApiProblem::internal(err.to_string()))?;

    Ok(Json(item_info_from_tx(&genesis_hash, &tx, block_num)?))
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

#[cfg(test)]
mod storage_handler_tests {
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};

    use axum::body::Body;
    use axum::http::{header, Request, StatusCode};
    use fleet_core::comms_handler::TcpTlsConfig;
    use fleet_core::configurations::DbMode;
    use fleet_core::db_utils::new_db;
    use fleet_core::interfaces::{BlockchainItemMeta, NodeType};
    use fleet_core::utils::{ApiKeys, RoutesPoWInfo};
    use fleet_core::Node;
    use http_body_util::BodyExt;
    use prime::crypto::sign_ed25519 as sign;
    use prime::primitives::asset::Asset;
    use prime::primitives::transaction::{GenesisTxHashSpec, OutPoint, Transaction, TxIn, TxOut};
    use prime::utils::transaction_utils::construct_item_create_tx;
    use serde_json::Value;
    use tower::ServiceExt;

    use crate::state::ApiState;
    use crate::v1::storage_router;

    async fn test_node() -> Node {
        let config = TcpTlsConfig::new_no_tls("127.0.0.1:0".parse().unwrap());
        Node::new(&config, 1, 1, NodeType::Storage, true, false)
            .await
            .expect("test node")
    }

    fn api_keys() -> ApiKeys {
        Arc::new(Mutex::new(BTreeMap::new()))
    }

    fn routes_pow() -> RoutesPoWInfo {
        Arc::new(Mutex::new(BTreeMap::new()))
    }

    /// Seeds an empty in-memory storage `SimpleDb`, then stores `entries` into it the
    /// same way `fleet_storage::store_complete_block` stores each transaction: keyed by
    /// its own hash, with a `BlockchainItemMeta::Tx` meta entry.
    async fn storage_state_with_txs(entries: &[(&str, &Transaction, u64, u32)]) -> ApiState {
        let db = Arc::new(Mutex::new(new_db(
            DbMode::InMemory,
            &fleet_storage::DB_SPEC,
            None,
            None,
        )));
        {
            let mut db = db.lock().unwrap();
            let mut batch = db.batch_writer();
            for (tx_hash, tx, block_num, tx_num) in entries {
                let meta = BlockchainItemMeta::Tx {
                    block_num: *block_num,
                    tx_num: *tx_num,
                };
                let tx_bin = bincode::serialize(tx).expect("tx serializes");
                let tx_json = serde_json::to_vec(tx).expect("tx json serializes");
                fleet_storage::put_to_block_chain(&mut batch, &meta, tx_hash, &tx_bin, &tx_json);
            }
            let batch = batch.done();
            db.write(batch).expect("write seeded entries");
        }
        ApiState::storage(test_node().await, db, api_keys(), routes_pow())
    }

    async fn body_json(response: axum::response::Response) -> Value {
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).expect("valid json body")
    }

    /// Builds a transfer tx moving an already-minted item, mirroring the shape
    /// `construct_rb_receive_payment_tx` produces (references the genesis output by
    /// `previous_out` rather than creating one).
    fn transfer_tx(genesis_hash: &str) -> Transaction {
        Transaction {
            inputs: vec![TxIn {
                previous_out: Some(OutPoint {
                    t_hash: genesis_hash.to_owned(),
                    n: 0,
                }),
                script_signature: Default::default(),
            }],
            outputs: vec![TxOut {
                value: Asset::item(1, Some(genesis_hash.to_owned()), None),
                locktime: 0,
                script_public_key: Some("recipient_address".to_owned()),
            }],
            version: 1,
            fees: vec![],
            druid_info: None,
        }
    }

    #[tokio::test]
    async fn get_item_info_storage_returns_genesis_metadata_for_a_transferred_item() {
        let (public_key, secret_key) = sign::gen_keypair();
        let create_tx = construct_item_create_tx(
            1,
            public_key,
            &secret_key,
            1000,
            GenesisTxHashSpec::Create,
            None,
            Some("some metadata".to_owned()),
        );
        let genesis_hash = "genesis_tx_hash";
        let transfer_hash = "transfer_tx_hash";
        let transfer = transfer_tx(genesis_hash);

        // The genesis entry lands in block 7, the transfer later in block 9 — the
        // handler must still report the genesis's own block_num/metadata for a
        // lookup keyed on genesis_hash, regardless of the later transfer.
        let state = storage_state_with_txs(&[
            (genesis_hash, &create_tx, 7, 0),
            (transfer_hash, &transfer, 9, 0),
        ])
        .await;
        let app = storage_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri(format!("/v1/items/{genesis_hash}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(body["genesis_hash"], genesis_hash);
        assert_eq!(body["metadata"], "some metadata");
        assert_eq!(body["total_amount"], 1000);
        assert_eq!(body["created"]["block_num"], 7);
        assert_eq!(body["created"]["tx_hash"], genesis_hash);
    }

    #[tokio::test]
    async fn get_item_info_storage_returns_404_for_unknown_genesis_hash() {
        let state = storage_state_with_txs(&[]).await;
        let app = storage_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/v1/items/unknown_hash")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            "application/problem+json"
        );

        let problem = body_json(response).await;
        assert_eq!(problem["status"], 404);
        assert!(problem["detail"].is_string());
    }

    #[tokio::test]
    async fn get_item_info_storage_returns_null_metadata_for_item_created_without_metadata() {
        let (public_key, secret_key) = sign::gen_keypair();
        let create_tx = construct_item_create_tx(
            1,
            public_key,
            &secret_key,
            500,
            GenesisTxHashSpec::Create,
            None,
            None,
        );
        let genesis_hash = "genesis_no_metadata_hash";

        let state = storage_state_with_txs(&[(genesis_hash, &create_tx, 1, 0)]).await;
        let app = storage_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri(format!("/v1/items/{genesis_hash}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(body["metadata"], Value::Null);
    }
}
