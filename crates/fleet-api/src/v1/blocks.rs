//! `GET /v1/blocks/latest` — the most recently stored block, if any.
//!
//! Reuses the same DB read the legacy `get_latest_block` handler used:
//! `fleet_core::db_utils::get_stored_value_from_db` at `LAST_BLOCK_HASH_KEY`.

use axum::extract::State;
use axum::Json;
use fleet_core::constants::LAST_BLOCK_HASH_KEY;
use fleet_core::db_utils::get_stored_value_from_db;
use serde::Serialize;
use serde_json::Value;
use utoipa::ToSchema;

use crate::error::ApiProblem;
use crate::state::ApiState;

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
