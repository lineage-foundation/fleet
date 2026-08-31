//! `GET /v1/transactions/status` and `POST /v1/transactions/status:query` — mempool
//! status for one or more transactions.
//!
//! Reuses the legacy `post_transaction_status` handler:
//! `MempoolApi::get_transaction_status(hashes)`, via a threaded call into the mempool
//! node.

use std::collections::BTreeMap;

use axum::extract::{Query, State};
use axum::Json;
use fleet_core::interfaces::{TxStatus, TxStatusType};
use fleet_core::threaded_call::make_threaded_call;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::error::ApiProblem;
use crate::state::ApiState;

/// Typed mirror of `fleet_core::interfaces::TxStatusType`.
#[derive(Debug, Serialize, ToSchema)]
pub enum TxStatusTypeResponse {
    Pending,
    Confirmed,
    Rejected,
}

impl From<TxStatusType> for TxStatusTypeResponse {
    fn from(status: TxStatusType) -> Self {
        match status {
            TxStatusType::Pending => Self::Pending,
            TxStatusType::Confirmed => Self::Confirmed,
            TxStatusType::Rejected => Self::Rejected,
        }
    }
}

/// Typed mirror of `fleet_core::interfaces::TxStatus`.
#[derive(Debug, Serialize, ToSchema)]
pub struct TxStatusResponse {
    pub status: TxStatusTypeResponse,
    pub timestamp: i64,
    pub additional_info: String,
}

impl From<TxStatus> for TxStatusResponse {
    fn from(status: TxStatus) -> Self {
        Self {
            status: status.status.into(),
            timestamp: status.timestamp,
            additional_info: status.additional_info,
        }
    }
}

/// Request body for the batch transaction-status lookup.
#[derive(Debug, Deserialize, ToSchema)]
pub struct HashesQuery {
    /// The transaction hashes to look up.
    pub hashes: Vec<String>,
}

async fn fetch_status(
    state: &ApiState,
    hashes: Vec<String>,
) -> Result<BTreeMap<String, TxStatusResponse>, ApiProblem> {
    let mut mempool_tx = state
        .mempool_calls_tx
        .clone()
        .ok_or_else(|| ApiProblem::internal("this node does not expose a mempool"))?;

    let status = make_threaded_call(
        &mut mempool_tx,
        move |c| c.get_transaction_status(hashes),
        "get_transaction_status",
    )
    .await?;

    Ok(status.into_iter().map(|(hash, status)| (hash, status.into())).collect())
}

/// Get mempool status for one or more transactions.
///
/// Repeat the `hash` query parameter for multiple hashes; use `POST
/// /v1/transactions/status:query` instead for large batches.
#[utoipa::path(
    get,
    path = "/v1/transactions/status",
    tag = "transactions",
    params(("hash" = Vec<String>, Query, description = "Transaction hashes to look up; repeat the parameter for multiple values")),
    responses(
        (status = 200, description = "Status for the requested transaction hashes, keyed by hash", body = BTreeMap<String, TxStatusResponse>),
        (status = 500, description = "The mempool node could not be reached", body = ApiProblem, content_type = "application/problem+json"),
    ),
)]
pub async fn get_transaction_status(
    State(state): State<ApiState>,
    Query(pairs): Query<Vec<(String, String)>>,
) -> Result<Json<BTreeMap<String, TxStatusResponse>>, ApiProblem> {
    let hashes = pairs
        .into_iter()
        .filter(|(key, _)| key == "hash")
        .map(|(_, value)| value)
        .collect();

    Ok(Json(fetch_status(&state, hashes).await?))
}

/// Batch-lookup mempool status for one or more transactions.
#[utoipa::path(
    post,
    path = "/v1/transactions/status:query",
    tag = "transactions",
    request_body = HashesQuery,
    responses(
        (status = 200, description = "Status for the requested transaction hashes, keyed by hash", body = BTreeMap<String, TxStatusResponse>),
        (status = 500, description = "The mempool node could not be reached", body = ApiProblem, content_type = "application/problem+json"),
    ),
)]
pub async fn query_transaction_status(
    State(state): State<ApiState>,
    Json(body): Json<HashesQuery>,
) -> Result<Json<BTreeMap<String, TxStatusResponse>>, ApiProblem> {
    Ok(Json(fetch_status(&state, body.hashes).await?))
}
