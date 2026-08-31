//! `GET /v1/blocks/latest`, `GET /v1/blocks/{num}` and `GET /v1/blocks` (batch).
//!
//! Reuses the same DB reads the legacy `get_latest_block`/`post_block_by_num` handlers
//! used: `fleet_core::db_utils::get_stored_value_from_db`, keyed for a given block
//! number via `fleet_core::db_utils::indexed_block_hash_key`.

use axum::extract::{Path, Query, State};
use axum::Json;
use fleet_core::constants::LAST_BLOCK_HASH_KEY;
use fleet_core::db_utils::{get_stored_value_from_db, indexed_block_hash_key};
use serde::Serialize;
use serde_json::Value;
use utoipa::ToSchema;

use crate::error::ApiProblem;
use crate::state::ApiState;
use crate::v1::blockchain::{item_to_entry_response, BlockchainEntryResponse};

/// The latest stored block.
///
/// The block payload is the pre-serialized JSON the storage node already keeps
/// alongside the binary-encoded block (`StoredSerializingBlock`, from `fleet-core`),
/// passed through unchanged.
#[derive(Debug, Serialize, ToSchema)]
pub struct LatestBlockResponse {
    #[schema(value_type = Object)]
    pub block: Value,
}

/// Get the most recently stored block.
#[utoipa::path(
    get,
    path = "/v1/blocks/latest",
    tag = "blocks",
    responses(
        (status = 200, description = "The latest stored block", body = LatestBlockResponse),
        (status = 404, description = "No block has been stored yet", body = ApiProblem, content_type = "application/problem+json"),
    ),
)]
pub async fn get_latest_block(State(state): State<ApiState>) -> Result<Json<LatestBlockResponse>, ApiProblem> {
    let db = state
        .db
        .clone()
        .ok_or_else(|| ApiProblem::internal("this node does not expose a blockchain database"))?;

    let item = get_stored_value_from_db(db, LAST_BLOCK_HASH_KEY)
        .ok_or_else(|| ApiProblem::not_found("no block has been stored yet"))?;

    let block: Value = serde_json::from_slice(&item.data_json)
        .map_err(|err| ApiProblem::internal(err.to_string()))?;

    Ok(Json(LatestBlockResponse { block }))
}

/// Get a single stored block by number.
#[utoipa::path(
    get,
    path = "/v1/blocks/{num}",
    tag = "blocks",
    params(("num" = u64, Path, description = "The block number")),
    responses(
        (status = 200, description = "The stored block entry", body = BlockchainEntryResponse),
        (status = 404, description = "No block has been stored at this number", body = ApiProblem, content_type = "application/problem+json"),
    ),
)]
pub async fn get_block_by_num(
    State(state): State<ApiState>,
    Path(num): Path<u64>,
) -> Result<Json<BlockchainEntryResponse>, ApiProblem> {
    let db = state
        .db
        .clone()
        .ok_or_else(|| ApiProblem::internal("this node does not expose a blockchain database"))?;

    let item = get_stored_value_from_db(db, indexed_block_hash_key(num))
        .ok_or_else(|| ApiProblem::not_found("no block has been stored at this number"))?;

    Ok(Json(item_to_entry_response(item)?))
}

/// Batch-lookup stored blocks by number.
///
/// Block numbers with no stored block are omitted from the response rather than
/// erroring; repeat the `num` query parameter for multiple numbers.
#[utoipa::path(
    get,
    path = "/v1/blocks",
    tag = "blocks",
    params(("num" = Vec<u64>, Query, description = "Block numbers to look up; repeat the parameter for multiple values")),
    responses(
        (status = 200, description = "The block entries found for the requested numbers", body = [BlockchainEntryResponse]),
    ),
)]
pub async fn get_blocks_batch(
    State(state): State<ApiState>,
    Query(pairs): Query<Vec<(String, String)>>,
) -> Result<Json<Vec<BlockchainEntryResponse>>, ApiProblem> {
    let db = state
        .db
        .clone()
        .ok_or_else(|| ApiProblem::internal("this node does not expose a blockchain database"))?;

    let nums = pairs
        .into_iter()
        .filter(|(key, _)| key == "num")
        .filter_map(|(_, value)| value.parse::<u64>().ok());

    let mut entries = Vec::new();
    for num in nums {
        if let Some(item) = get_stored_value_from_db(db.clone(), indexed_block_hash_key(num)) {
            entries.push(item_to_entry_response(item)?);
        }
    }

    Ok(Json(entries))
}
