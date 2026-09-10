//! `GET /v1/mining/current-block` — the latest block received for mining, for a node
//! that mines (miner, solo miner).
//!
//! Reuses the legacy `get_current_mining_block` handler:
//! `state.current_block.lock().await.clone()`.

use axum::extract::State;
use axum::Json;
use serde::Serialize;
use serde_json::Value;
use utoipa::ToSchema;

use crate::error::ApiProblem;
use crate::state::ApiState;
use crate::v1::difficulty::{decode_difficulty_target, DifficultyTarget};

/// The latest block received for mining, if any.
#[derive(Debug, Serialize, ToSchema)]
pub struct CurrentBlockResponse {
    /// The current mining block (`fleet_core::interfaces::BlockPoWReceived`), passed
    /// through as JSON unchanged, or `null` if no block has been received yet.
    #[schema(value_type = Object, nullable = true)]
    pub block: Option<Value>,
    /// The current block header's committed PoW target (`bits`), decoded into its
    /// compact `nBits` and expanded 256-bit forms. `null` when no block has been
    /// received yet or the header carries no committed target (legacy `bits == 0`).
    pub difficulty_target: Option<DifficultyTarget>,
}

/// Get the latest block received for mining.
#[utoipa::path(
    get,
    path = "/v1/mining/current-block",
    tag = "mining",
    responses(
        (status = 200, description = "The current mining block, or `null` if none has been received yet", body = CurrentBlockResponse),
        (status = 500, description = "This node does not mine", body = ApiProblem, content_type = "application/problem+json"),
    ),
)]
pub async fn get_current_block(State(state): State<ApiState>) -> Result<Json<CurrentBlockResponse>, ApiProblem> {
    let current_block = state
        .current_block
        .clone()
        .ok_or_else(|| ApiProblem::internal("this node does not mine"))?;

    let data = current_block.lock().await.clone();
    let difficulty_target = data
        .as_ref()
        .and_then(|received| decode_difficulty_target(received.block.bits));
    let block = data
        .map(|block| serde_json::to_value(&block))
        .transpose()
        .map_err(|err| ApiProblem::internal(err.to_string()))?;

    Ok(Json(CurrentBlockResponse {
        block,
        difficulty_target,
    }))
}
