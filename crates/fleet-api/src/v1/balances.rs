//! `GET /v1/balances` and `POST /v1/balances/query` — UTXO balances for one or more
//! addresses.
//!
//! Reuses the legacy `post_fetch_utxo_balance` handler:
//! `MempoolApi::get_committed_utxo_tracked_set().get_balance_for_addresses(&addresses)`,
//! via a threaded call into the mempool node. The resulting `TrackedUtxoBalance`
//! (`fleet_core::tracked_utxo`) is passed through as JSON unchanged (its fields are
//! private, so it's serialized as-is rather than remapped field-by-field), mirroring
//! the legacy embed-as-JSON behaviour.

use axum::extract::{Query, State};
use axum::Json;
use fleet_core::threaded_call::make_threaded_call;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;

use crate::error::ApiProblem;
use crate::state::ApiState;

/// UTXO balance for the requested addresses.
#[derive(Debug, Serialize, ToSchema)]
pub struct BalancesResponse {
    /// The combined asset totals and per-address outpoint breakdown, as returned by
    /// the mempool's tracked UTXO set (`fleet_core::tracked_utxo::TrackedUtxoBalance`).
    #[schema(value_type = Object)]
    pub balance: Value,
}

/// Request body for the batch balances lookup.
#[derive(Debug, Deserialize, ToSchema)]
pub struct AddressesQuery {
    /// The addresses to look up.
    pub addresses: Vec<String>,
}

async fn fetch_balance(state: &ApiState, addresses: Vec<String>) -> Result<BalancesResponse, ApiProblem> {
    let mut mempool_tx = state
        .mempool_calls_tx
        .clone()
        .ok_or_else(|| ApiProblem::internal("this node does not expose a mempool"))?;

    let balance = make_threaded_call(
        &mut mempool_tx,
        move |c| c.get_committed_utxo_tracked_set().get_balance_for_addresses(&addresses),
        "get_balance_for_addresses",
    )
    .await?;

    let balance = serde_json::to_value(&balance).map_err(|err| ApiProblem::internal(err.to_string()))?;
    Ok(BalancesResponse { balance })
}

/// Get UTXO balances for one or more addresses.
///
/// Repeat the `address` query parameter for multiple addresses; use `POST
/// /v1/balances/query` instead for large batches.
#[utoipa::path(
    get,
    path = "/v1/balances",
    tag = "balances",
    params(("address" = Vec<String>, Query, description = "Addresses to look up; repeat the parameter for multiple values")),
    responses(
        (status = 200, description = "UTXO balance for the requested addresses", body = BalancesResponse),
        (status = 500, description = "The mempool node could not be reached", body = ApiProblem, content_type = "application/problem+json"),
    ),
)]
pub async fn get_balances(
    State(state): State<ApiState>,
    Query(pairs): Query<Vec<(String, String)>>,
) -> Result<Json<BalancesResponse>, ApiProblem> {
    let addresses = pairs
        .into_iter()
        .filter(|(key, _)| key == "address")
        .map(|(_, value)| value)
        .collect();

    Ok(Json(fetch_balance(&state, addresses).await?))
}

/// Batch-lookup UTXO balances for one or more addresses.
#[utoipa::path(
    post,
    path = "/v1/balances/query",
    tag = "balances",
    request_body = AddressesQuery,
    responses(
        (status = 200, description = "UTXO balance for the requested addresses", body = BalancesResponse),
        (status = 500, description = "The mempool node could not be reached", body = ApiProblem, content_type = "application/problem+json"),
    ),
)]
pub async fn query_balances(
    State(state): State<ApiState>,
    Json(body): Json<AddressesQuery>,
) -> Result<Json<BalancesResponse>, ApiProblem> {
    Ok(Json(fetch_balance(&state, body.addresses).await?))
}
