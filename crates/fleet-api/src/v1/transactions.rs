//! `GET /v1/transactions/status`, `POST /v1/transactions/status:query` — mempool status
//! for one or more transactions; `GET /v1/transactions/outgoing` — this node's
//! constructed-and-sent transactions; `POST /v1/transactions` — construct and submit
//! transactions to the mempool.
//!
//! Reuses the legacy `post_transaction_status` handler:
//! `MempoolApi::get_transaction_status(hashes)`, via a threaded call into the mempool
//! node; the legacy `get_outgoing_txs` handler: `WalletDb::get_outgoing_txs()`; and the
//! legacy `post_create_transactions` handler: `MempoolApi::receive_transactions(..)`.

use std::collections::BTreeMap;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::Json;
use fleet_core::interfaces::{TxStatus, TxStatusType};
use fleet_core::threaded_call::make_threaded_call;
use fleet_wallet::WalletDbError;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tw_chain::primitives::transaction::Transaction;
use utoipa::ToSchema;

use super::tx_convert::{construct_ctx_map, to_transaction, CreateTransaction};
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

/// This node's outgoing (constructed-and-sent) transactions, keyed by hash.
#[derive(Debug, Serialize, ToSchema)]
pub struct OutgoingTxsResponse {
    /// `[hash, transaction]` pairs (`tw_chain::primitives::transaction::Transaction`),
    /// passed through as JSON unchanged, mirroring the legacy embed-as-JSON behaviour.
    #[schema(value_type = Vec<Object>)]
    pub transactions: Value,
}

/// Get this node's outgoing (constructed-and-sent) transactions.
///
/// An empty wallet with no outgoing transactions yet returns an empty list rather than
/// an error, unlike the legacy handler (which surfaced the "no key in the DB yet" case
/// as a `500`).
#[utoipa::path(
    get,
    path = "/v1/transactions/outgoing",
    tag = "transactions",
    responses(
        (status = 200, description = "This node's outgoing transactions", body = OutgoingTxsResponse),
        (status = 500, description = "This node does not expose a wallet, or the wallet DB could not be read", body = ApiProblem, content_type = "application/problem+json"),
    ),
)]
pub async fn get_outgoing_txs(State(state): State<ApiState>) -> Result<Json<OutgoingTxsResponse>, ApiProblem> {
    let wallet_db = state
        .wallet_db
        .clone()
        .ok_or_else(|| ApiProblem::internal("this node does not expose a wallet"))?;

    let txs = match wallet_db.get_outgoing_txs() {
        Ok(txs) => txs,
        Err(WalletDbError::OutgoingTxMissingError) => Vec::new(),
        Err(err) => return Err(ApiProblem::internal(err.to_string())),
    };

    let transactions = serde_json::to_value(&txs).map_err(|err| ApiProblem::internal(err.to_string()))?;
    Ok(Json(OutgoingTxsResponse { transactions }))
}

/// Request body for `POST /v1/transactions`.
#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateTransactionsRequest {
    /// The transactions to construct and submit.
    pub transactions: Vec<CreateTransaction>,
}

/// Response body for `POST /v1/transactions`.
#[derive(Debug, Serialize, ToSchema)]
pub struct CreateTransactionsResponse {
    /// Per-constructed-transaction summary keyed by tx hash: `[output_address, asset]`.
    #[schema(value_type = Object)]
    pub transactions: Value,
}

/// Construct one or more transactions and submit them to the mempool.
#[utoipa::path(
    post,
    path = "/v1/transactions",
    tag = "transactions",
    request_body = CreateTransactionsRequest,
    responses(
        (status = 201, description = "The transaction(s) were accepted by the mempool", body = CreateTransactionsResponse),
        (status = 400, description = "One or more transactions were malformed", body = ApiProblem, content_type = "application/problem+json"),
        (status = 500, description = "The mempool node could not be reached, or rejected the transaction(s)", body = ApiProblem, content_type = "application/problem+json"),
    ),
)]
pub async fn post_create_transactions(
    State(state): State<ApiState>,
    Json(body): Json<CreateTransactionsRequest>,
) -> Result<(StatusCode, Json<CreateTransactionsResponse>), ApiProblem> {
    let mut mempool_tx = state
        .mempool_calls_tx
        .clone()
        .ok_or_else(|| ApiProblem::internal("this node does not expose a mempool"))?;

    let transactions: Vec<Transaction> = body
        .transactions
        .into_iter()
        .map(to_transaction)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| ApiProblem::bad_request(err.0))?;

    let ctx_map = construct_ctx_map(&transactions);

    let resp = make_threaded_call(
        &mut mempool_tx,
        move |c| c.receive_transactions(transactions),
        "receive_transactions",
    )
    .await?;

    if !resp.success {
        return Err(ApiProblem::internal(resp.reason));
    }

    let transactions = serde_json::to_value(&ctx_map).map_err(|err| ApiProblem::internal(err.to_string()))?;

    Ok((StatusCode::CREATED, Json(CreateTransactionsResponse { transactions })))
}
