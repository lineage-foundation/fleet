//! `GET /v1/wallet` and `GET /v1/wallet/keypairs` — wallet balance/transaction info and
//! key-pair export for a node with a wallet (user, miner).
//!
//! Reuses the legacy `get_wallet_info`/`get_export_keypairs` handlers:
//! `WalletDb::get_fund_store_err()` plus the address/outpoint/running-total assembly for
//! wallet info, and `WalletDb::{get_known_addresses, get_address_store}` for the keypair
//! export.

use std::collections::BTreeMap;

use axum::extract::{Query, State};
use axum::Json;
use fleet_core::constants::D_DISPLAY_PLACES;
use fleet_core::interfaces::{AddressesWithOutPoints, OutPointData};
use fleet_wallet::AddressStoreHex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;

use crate::error::ApiProblem;
use crate::state::ApiState;

/// Balance and transaction info for this node's wallet.
#[derive(Debug, Serialize, ToSchema)]
pub struct WalletInfoResponse {
    /// Total tokens held, in display units.
    pub running_total: f64,
    /// Total tokens held, in raw token units.
    pub running_total_tokens: u64,
    /// Tokens currently locked (e.g. immature coinbase), in display units.
    pub locked_total: f64,
    /// Tokens currently locked, in raw token units.
    pub locked_total_tokens: u64,
    /// Tokens available to spend, in display units.
    pub available_total: f64,
    /// Tokens available to spend, in raw token units.
    pub available_total_tokens: u64,
    /// Item-asset totals, keyed by genesis hash.
    pub item_total: BTreeMap<String, u64>,
    /// Outpoints (with their held asset), keyed by owning address
    /// (`fleet_core::interfaces::AddressesWithOutPoints`), passed through as JSON
    /// unchanged (its element type has private fields, so it's serialized as-is rather
    /// than remapped field-by-field), mirroring the legacy embed-as-JSON behaviour.
    #[schema(value_type = Object)]
    pub addresses: Value,
}

/// Query parameters for `GET /v1/wallet`.
///
/// The legacy handler overloaded a single `extra: Option<String>` request param to mean
/// either "spent" or a page number; this splits that into two typed params. `spent`
/// takes precedence over `page` when both are set, matching the legacy handler's
/// `Some("spent")` branch taking precedence over falling through to a numeric parse.
#[derive(Debug, Deserialize, ToSchema)]
pub struct WalletInfoQuery {
    /// Which page of `transaction_pages` to return the outpoints from; defaults to the
    /// unpaged full transaction set when omitted.
    pub page: Option<u64>,
    /// When `true`, return spent transactions instead of the (paged or unpaged) unspent
    /// set.
    pub spent: Option<bool>,
}

/// Get balance and transaction info for this node's wallet.
#[utoipa::path(
    get,
    path = "/v1/wallet",
    tag = "wallet",
    params(
        ("page" = Option<u64>, Query, description = "Page of transaction_pages to return outpoints from"),
        ("spent" = Option<bool>, Query, description = "Return spent transactions instead of the unspent set"),
    ),
    responses(
        (status = 200, description = "Balance and transaction info for this node's wallet", body = WalletInfoResponse),
        (status = 500, description = "This node does not expose a wallet, or the wallet DB could not be read", body = ApiProblem, content_type = "application/problem+json"),
    ),
)]
pub async fn get_wallet_info(
    State(state): State<ApiState>,
    Query(query): Query<WalletInfoQuery>,
) -> Result<Json<WalletInfoResponse>, ApiProblem> {
    let wallet_db = state
        .wallet_db
        .clone()
        .ok_or_else(|| ApiProblem::internal("this node does not expose a wallet"))?;

    let mut fund_store = wallet_db
        .get_fund_store_err()
        .map_err(|err| ApiProblem::internal(err.to_string()))?;

    let txs = if query.spent == Some(true) {
        fund_store.spent_transactions().clone()
    } else if let Some(page) = query.page {
        fund_store.transaction_pages(page as usize).clone()
    } else {
        fund_store.transactions().clone()
    };

    let mut addresses: AddressesWithOutPoints = AddressesWithOutPoints::new();
    for (out_point, asset) in txs {
        addresses
            .entry(wallet_db.get_transaction_address(&out_point))
            .or_default()
            .push(OutPointData::new(out_point, asset));
    }

    let locked_coinbase = wallet_db.get_locked_coinbase().await;
    let total = fund_store.running_total().clone();
    let available = {
        fund_store.filter_locked_coinbase(&locked_coinbase);
        fund_store.running_total().clone()
    };

    let addresses = serde_json::to_value(&addresses).map_err(|err| ApiProblem::internal(err.to_string()))?;

    Ok(Json(WalletInfoResponse {
        running_total: total.tokens.0 as f64 / D_DISPLAY_PLACES,
        running_total_tokens: total.tokens.0,
        locked_total: (total.tokens.0 - available.tokens.0) as f64 / D_DISPLAY_PLACES,
        locked_total_tokens: total.tokens.0 - available.tokens.0,
        available_total: available.tokens.0 as f64 / D_DISPLAY_PLACES,
        available_total_tokens: available.tokens.0,
        item_total: total.items,
        addresses,
    }))
}

/// Exported key-pairs, keyed by payment address.
#[derive(Debug, Serialize, ToSchema)]
pub struct KeypairsResponse {
    /// Hex-encoded key-pairs, keyed by payment address
    /// (`fleet_wallet::AddressStoreHex`), passed through as JSON unchanged, mirroring
    /// the legacy `Addresses` embed-as-JSON behaviour.
    #[schema(value_type = Object)]
    pub addresses: Value,
}

/// Export this node's known key-pairs.
///
/// Sensitive: this returns private keys. Protect this route with an api-key entry.
#[utoipa::path(
    get,
    path = "/v1/wallet/keypairs",
    tag = "wallet",
    responses(
        (status = 200, description = "Hex-encoded key-pairs, keyed by payment address", body = KeypairsResponse),
        (status = 500, description = "This node does not expose a wallet", body = ApiProblem, content_type = "application/problem+json"),
    ),
    security(("api_key" = [])),
)]
pub async fn get_keypairs(State(state): State<ApiState>) -> Result<Json<KeypairsResponse>, ApiProblem> {
    let wallet_db = state
        .wallet_db
        .clone()
        .ok_or_else(|| ApiProblem::internal("this node does not expose a wallet"))?;

    let mut addresses: BTreeMap<String, AddressStoreHex> = BTreeMap::new();
    for addr in wallet_db.get_known_addresses() {
        let store = wallet_db.get_address_store(&addr);
        addresses.insert(addr, store.into());
    }

    let addresses = serde_json::to_value(&addresses).map_err(|err| ApiProblem::internal(err.to_string()))?;
    Ok(Json(KeypairsResponse { addresses }))
}
