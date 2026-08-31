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
use fleet_wallet::{AddressStore, AddressStoreHex};
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
/// either "spent" or a page number; this splits that into two typed params. With separate
/// params both can be set at once, so we deliberately let `spent` take precedence over
/// `page` (legacy never had to choose, since `extra` carried only one value).
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

/// A newly generated payment address.
#[derive(Debug, Serialize, ToSchema)]
pub struct NewAddressResponse {
    /// The newly generated payment address.
    pub address: String,
}

/// Generate and return a new payment address for this node's wallet.
#[utoipa::path(
    post,
    path = "/v1/wallet/addresses",
    tag = "wallet",
    responses(
        (status = 201, description = "A newly generated payment address", body = NewAddressResponse),
        (status = 500, description = "This node does not expose a wallet", body = ApiProblem, content_type = "application/problem+json"),
    ),
    security(("api_key" = [])),
)]
pub async fn post_new_address(
    State(state): State<ApiState>,
) -> Result<(axum::http::StatusCode, Json<NewAddressResponse>), ApiProblem> {
    let mut wallet_db = state
        .wallet_db
        .clone()
        .ok_or_else(|| ApiProblem::internal("this node does not expose a wallet"))?;
    let (address, _) = wallet_db.generate_payment_address();
    Ok((axum::http::StatusCode::CREATED, Json(NewAddressResponse { address })))
}

/// Request body for `PUT /v1/wallet/passphrase`.
#[derive(Debug, Deserialize, ToSchema)]
pub struct ChangePassphraseRequest {
    /// The wallet's current passphrase.
    pub old_passphrase: String,
    /// The passphrase to change to.
    pub new_passphrase: String,
}

/// Change this node's wallet passphrase.
#[utoipa::path(
    put,
    path = "/v1/wallet/passphrase",
    tag = "wallet",
    request_body = ChangePassphraseRequest,
    responses(
        (status = 204, description = "Passphrase changed"),
        (status = 400, description = "The new passphrase was blank", body = ApiProblem, content_type = "application/problem+json"),
        (status = 500, description = "This node does not expose a wallet, or the passphrase change failed", body = ApiProblem, content_type = "application/problem+json"),
    ),
    security(("api_key" = [])),
)]
pub async fn put_passphrase(
    State(state): State<ApiState>,
    Json(body): Json<ChangePassphraseRequest>,
) -> Result<axum::http::StatusCode, ApiProblem> {
    let mut wallet_db = state
        .wallet_db
        .clone()
        .ok_or_else(|| ApiProblem::internal("this node does not expose a wallet"))?;
    if body.new_passphrase.is_empty() {
        return Err(ApiProblem::bad_request("new passphrase must not be blank"));
    }
    wallet_db
        .change_wallet_passphrase(body.old_passphrase, body.new_passphrase)
        .await
        .map_err(|err| ApiProblem::internal(err.to_string()))?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

/// Request body for `POST /v1/wallet/keypairs`.
#[derive(Debug, Deserialize, ToSchema)]
pub struct ImportKeypairsRequest {
    /// Hex-encoded key-pairs to import, keyed by payment address (mirrors the legacy
    /// `Addresses` request body / `KeypairsResponse`).
    #[schema(value_type = Object)]
    pub addresses: BTreeMap<String, AddressStoreHex>,
}

/// The payment addresses imported by `POST /v1/wallet/keypairs`.
#[derive(Debug, Serialize, ToSchema)]
pub struct ImportKeypairsResponse {
    /// The payment addresses that were imported.
    pub imported: Vec<String>,
}

/// Import key-pairs into this node's wallet, then request a running-total refresh from
/// the UTXO set for the imported addresses.
#[utoipa::path(
    post,
    path = "/v1/wallet/keypairs",
    tag = "wallet",
    request_body = ImportKeypairsRequest,
    responses(
        (status = 201, description = "The payment addresses that were imported", body = ImportKeypairsResponse),
        (status = 400, description = "One of the key-pairs was not valid hex", body = ApiProblem, content_type = "application/problem+json"),
        (status = 500, description = "This node does not expose a wallet, the key-pairs could not be saved, or the running-total refresh could not be requested", body = ApiProblem, content_type = "application/problem+json"),
    ),
    security(("api_key" = [])),
)]
pub async fn post_import_keypairs(
    State(state): State<ApiState>,
    Json(body): Json<ImportKeypairsRequest>,
) -> Result<(axum::http::StatusCode, Json<ImportKeypairsResponse>), ApiProblem> {
    let wallet_db = state
        .wallet_db
        .clone()
        .ok_or_else(|| ApiProblem::internal("this node does not expose a wallet"))?;

    let imported: Vec<String> = body.addresses.keys().cloned().collect();

    // Convert every entry before saving any of them, so a bad hex further down the
    // request never leaves earlier entries persisted behind a 400 response.
    let mut converted: Vec<(String, AddressStore)> = Vec::with_capacity(body.addresses.len());
    for (addr, hex) in body.addresses {
        let store =
            AddressStore::try_from_hex_store(hex).map_err(|err| ApiProblem::bad_request(err.to_string()))?;
        converted.push((addr, store));
    }

    for (addr, store) in converted {
        wallet_db
            .save_address_to_wallet(addr, store)
            .map_err(|err| ApiProblem::internal(err.to_string()))?;
    }

    inject_running_total_refresh(wallet_node(&state), imported.clone())?;

    Ok((axum::http::StatusCode::CREATED, Json(ImportKeypairsResponse { imported })))
}

/// Request body for `POST /v1/wallet/running-total:refresh`.
#[derive(Debug, Deserialize, ToSchema)]
pub struct RunningTotalRefreshRequest {
    /// Refresh every known address (ignores `addresses` when true).
    #[serde(default)]
    pub all: bool,
    /// The specific addresses to refresh when `all` is false.
    #[serde(default)]
    pub addresses: Vec<String>,
}

/// Pick the node that owns the wallet for running-total refreshes: a miner paired with
/// an embedded user node routes these through that user node (`aux_node`), exactly as the
/// legacy `miner_node_with_user_routes` did; a solo miner or a standalone user node has no
/// `aux_node`, so it uses the node itself.
fn wallet_node(state: &ApiState) -> &fleet_core::Node {
    state.aux_node.as_ref().unwrap_or(&state.node)
}

/// Inject a "refresh running total from the UTXO set" event into the given node, branching
/// on node type exactly as the legacy import/update-running-total handlers did.
///
/// `inject_next_event` is generic (`data: impl Serialize`), so each arm passes its
/// concrete request type directly — do NOT box into a trait object.
fn inject_running_total_refresh(node: &fleet_core::Node, addresses: Vec<String>) -> Result<(), ApiProblem> {
    use fleet_core::interfaces::{MineApiRequest, MineRequest, NodeType, UserApiRequest, UserRequest, UtxoFetchType};
    let from = node.local_address();
    match node.get_node_type() {
        NodeType::Miner => node
            .inject_next_event(
                from,
                MineRequest::MinerApi(MineApiRequest::RequestUTXOSet(UtxoFetchType::AnyOf(addresses))),
            )
            .map_err(|err| ApiProblem::internal(err.to_string())),
        NodeType::User => node
            .inject_next_event(
                from,
                UserRequest::UserApi(UserApiRequest::UpdateWalletFromUtxoSet {
                    address_list: UtxoFetchType::AnyOf(addresses),
                }),
            )
            .map_err(|err| ApiProblem::internal(err.to_string())),
        _ => Err(ApiProblem::internal("this node cannot refresh a running total")),
    }
}

/// Request a running-total refresh from the UTXO set for this node's wallet.
#[utoipa::path(
    post,
    path = "/v1/wallet/running-total:refresh",
    tag = "wallet",
    request_body = RunningTotalRefreshRequest,
    responses(
        (status = 202, description = "The running-total refresh was requested"),
        (status = 400, description = "No addresses to refresh were resolved", body = ApiProblem, content_type = "application/problem+json"),
        (status = 500, description = "This node does not expose a wallet, or the running-total refresh could not be requested", body = ApiProblem, content_type = "application/problem+json"),
    ),
    security(("api_key" = [])),
)]
pub async fn post_running_total_refresh(
    State(state): State<ApiState>,
    Json(body): Json<RunningTotalRefreshRequest>,
) -> Result<axum::http::StatusCode, ApiProblem> {
    let wallet_db = state
        .wallet_db
        .clone()
        .ok_or_else(|| ApiProblem::internal("this node does not expose a wallet"))?;

    let list = if body.all {
        wallet_db.get_known_addresses()
    } else {
        body.addresses
    };
    if list.is_empty() {
        return Err(ApiProblem::bad_request("no addresses to refresh"));
    }

    inject_running_total_refresh(wallet_node(&state), list)?;

    Ok(axum::http::StatusCode::ACCEPTED)
}
