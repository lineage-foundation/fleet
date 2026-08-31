//! `GET /v1/supply` — total and issued token supply.
//!
//! Reuses the legacy `get_total_supply`/`get_issued_supply` handlers: `total` is the
//! `TOTAL_TOKENS` constant (`fleet_core::constants`, re-exported from `tw_chain`);
//! `issued` comes from a threaded call into the mempool node's
//! `MempoolApi::get_issued_supply`.

use axum::extract::State;
use axum::Json;
use fleet_core::constants::TOTAL_TOKENS;
use fleet_core::threaded_call::make_threaded_call;
use serde::Serialize;
use utoipa::ToSchema;

use crate::error::ApiProblem;
use crate::state::ApiState;

/// Total and issued token supply.
#[derive(Debug, Serialize, ToSchema)]
pub struct SupplyResponse {
    /// The fixed total token supply.
    pub total: u64,
    /// The currently issued token supply.
    pub issued: u64,
}

/// Get the total and issued token supply.
#[utoipa::path(
    get,
    path = "/v1/supply",
    tag = "supply",
    responses(
        (status = 200, description = "Total and issued token supply", body = SupplyResponse),
        (status = 500, description = "The mempool node could not be reached", body = ApiProblem, content_type = "application/problem+json"),
    ),
)]
pub async fn get_supply(State(state): State<ApiState>) -> Result<Json<SupplyResponse>, ApiProblem> {
    let mut mempool_tx = state
        .mempool_calls_tx
        .clone()
        .ok_or_else(|| ApiProblem::internal("this node does not expose a mempool"))?;

    let issued = make_threaded_call(&mut mempool_tx, |c| c.get_issued_supply(), "get_issued_supply")
        .await?
        .0;

    Ok(Json(SupplyResponse {
        total: TOTAL_TOKENS,
        issued,
    }))
}
