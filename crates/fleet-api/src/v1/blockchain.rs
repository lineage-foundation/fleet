//! `GET /v1/blockchain-entries/{key}` and `POST /v1/blockchain-entries/query` — direct
//! blockchain-DB reads by raw storage key.
//!
//! Reuses the same DB read the legacy `post_blockchain_entry_by_key` handler used:
//! `fleet_core::db_utils::get_stored_value_from_db`. `/v1/blocks/{num}` and
//! `/v1/blocks` (batch) in `v1::blocks` build on the same `BlockchainEntryResponse`
//! DTO and `item_to_entry_response` conversion, keyed via `indexed_block_hash_key`
//! instead of a raw key.

use axum::extract::{Path, State};
use axum::Json;
use fleet_core::db_utils::get_stored_value_from_db;
use fleet_core::interfaces::{BlockchainItem, BlockchainItemMeta};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;

use crate::error::ApiProblem;
use crate::state::ApiState;

/// A single stored blockchain entry (a block or a transaction), as kept in the
/// blockchain DB.
#[derive(Debug, Serialize, ToSchema)]
pub struct BlockchainEntryResponse {
    /// The entry's raw storage key (a transaction hash, or an indexed block key).
    pub key: String,
    /// Whether this entry is a block or a transaction, with its position metadata.
    pub item_meta: BlockchainItemMetaResponse,
    /// The entry's JSON payload, as stored alongside the binary-encoded data.
    #[schema(value_type = Object)]
    pub data: Value,
}

/// Typed mirror of `fleet_core::interfaces::BlockchainItemMeta`.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BlockchainItemMetaResponse {
    Block { block_num: u64, tx_len: u32 },
    Tx { block_num: u64, tx_num: u32 },
}

impl From<BlockchainItemMeta> for BlockchainItemMetaResponse {
    fn from(meta: BlockchainItemMeta) -> Self {
        match meta {
            BlockchainItemMeta::Block { block_num, tx_len } => Self::Block { block_num, tx_len },
            BlockchainItemMeta::Tx { block_num, tx_num } => Self::Tx { block_num, tx_num },
        }
    }
}

/// Request body for the batch blockchain-entries lookup.
#[derive(Debug, Deserialize, ToSchema)]
pub struct KeysQuery {
    /// The raw storage keys to look up.
    pub keys: Vec<String>,
}

/// Convert a raw DB item into its typed API response, parsing its stored JSON
/// payload.
pub(crate) fn item_to_entry_response(item: BlockchainItem) -> Result<BlockchainEntryResponse, ApiProblem> {
    let data: Value =
        serde_json::from_slice(&item.data_json).map_err(|err| ApiProblem::internal(err.to_string()))?;

    Ok(BlockchainEntryResponse {
        key: String::from_utf8_lossy(&item.key).into_owned(),
        item_meta: item.item_meta.into(),
        data,
    })
}

/// Get a single blockchain DB entry by its raw storage key.
#[utoipa::path(
    get,
    path = "/v1/blockchain-entries/{key}",
    tag = "blockchain-entries",
    params(("key" = String, Path, description = "The raw storage key")),
    responses(
        (status = 200, description = "The stored entry", body = BlockchainEntryResponse),
        (status = 404, description = "No entry stored at this key", body = ApiProblem, content_type = "application/problem+json"),
    ),
)]
pub async fn get_blockchain_entry(
    State(state): State<ApiState>,
    Path(key): Path<String>,
) -> Result<Json<BlockchainEntryResponse>, ApiProblem> {
    let db = state
        .db
        .clone()
        .ok_or_else(|| ApiProblem::internal("this node does not expose a blockchain database"))?;

    let item =
        get_stored_value_from_db(db, key).ok_or_else(|| ApiProblem::not_found("no entry stored at this key"))?;

    Ok(Json(item_to_entry_response(item)?))
}

/// Batch-lookup blockchain DB entries by raw storage key.
///
/// Keys with no stored entry are omitted from the response rather than erroring.
#[utoipa::path(
    post,
    path = "/v1/blockchain-entries/query",
    tag = "blockchain-entries",
    request_body = KeysQuery,
    responses(
        (status = 200, description = "The entries found for the requested keys", body = [BlockchainEntryResponse]),
    ),
)]
pub async fn query_blockchain_entries(
    State(state): State<ApiState>,
    Json(body): Json<KeysQuery>,
) -> Result<Json<Vec<BlockchainEntryResponse>>, ApiProblem> {
    let db = state
        .db
        .clone()
        .ok_or_else(|| ApiProblem::internal("this node does not expose a blockchain database"))?;

    let mut entries = Vec::with_capacity(body.keys.len());
    for key in body.keys {
        if let Some(item) = get_stored_value_from_db(db.clone(), key) {
            entries.push(item_to_entry_response(item)?);
        }
    }

    Ok(Json(entries))
}
