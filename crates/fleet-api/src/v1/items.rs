//! `POST /v1/items` — create item-asset transactions.
//!
//! Reuses the legacy `post_create_item_asset` handler (mempool: signs and submits an
//! item-asset creation transaction directly, via `MempoolApi::create_item_asset_tx` +
//! `receive_transactions`) and `post_create_item_asset_user` handler (user: injects a
//! `SendCreateItemRequest` event for the node to construct and sign the transaction
//! itself, via `crate::v1::wallet::wallet_node`).

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use fleet_core::interfaces::{MempoolError, UserApiRequest, UserRequest};
use fleet_core::threaded_call::make_threaded_call;
use serde::{Deserialize, Serialize};
use tw_chain::primitives::asset::{Asset, ItemAsset};
use tw_chain::primitives::transaction::GenesisTxHashSpec;
use utoipa::ToSchema;

use super::asset::ApiAsset;
use crate::error::ApiProblem;
use crate::state::ApiState;

/// Request body for `POST /v1/items`.
#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateItemRequest {
    /// The number of items to create.
    pub item_amount: u64,
    #[schema(value_type = String)]
    pub genesis_hash_spec: GenesisTxHashSpec,
    /// Optional item metadata.
    #[serde(default)]
    pub metadata: Option<String>,
    /// Required on a mempool node (the client-signed create); ignored on a user node.
    #[serde(default)]
    pub script_public_key: Option<String>,
    /// Required on a mempool node (the client-signed create); ignored on a user node.
    #[serde(default)]
    pub public_key: Option<String>,
    /// Required on a mempool node (the client-signed create); ignored on a user node.
    #[serde(default)]
    pub signature: Option<String>,
}

/// Response body for a mempool-node item-asset creation.
#[derive(Debug, Serialize, ToSchema)]
pub struct CreateItemResponse {
    /// The created item asset.
    pub asset: ApiAsset,
    /// The address the item was created against.
    pub to_address: String,
    /// The hash of the created item-asset transaction.
    pub tx_hash: String,
}

/// Response body for a user-node item-asset creation request.
#[derive(Debug, Serialize, ToSchema)]
pub struct CreateItemAcceptedResponse {
    /// The number of items requested.
    pub item_amount: u64,
}

/// Create an item asset.
///
/// On a mempool node, this signs and submits the item-asset creation transaction
/// directly (`script_public_key`/`public_key`/`signature` are required) and returns
/// `201` with the created asset. On a user node, this injects a creation request for
/// the node to construct and sign itself, and returns `202`.
#[utoipa::path(
    post,
    path = "/v1/items",
    tag = "items",
    request_body = CreateItemRequest,
    responses(
        (status = 201, description = "Item asset created on the mempool", body = CreateItemResponse),
        (status = 202, description = "Item-asset creation accepted by the user node", body = CreateItemAcceptedResponse),
        (status = 400, description = "Missing required fields for a mempool-node create", body = ApiProblem, content_type = "application/problem+json"),
        (status = 500, description = "The node could not be reached, or rejected the request", body = ApiProblem, content_type = "application/problem+json"),
    ),
    security(("api_key" = [])),
)]
pub async fn post_create_item(
    State(state): State<ApiState>,
    Json(body): Json<CreateItemRequest>,
) -> Result<axum::response::Response, ApiProblem> {
    let CreateItemRequest {
        item_amount,
        genesis_hash_spec,
        metadata,
        script_public_key,
        public_key,
        signature,
    } = body;

    if let Some(mut mempool_tx) = state.mempool_calls_tx.clone() {
        let (script_public_key, public_key, signature) = match (script_public_key, public_key, signature) {
            (Some(spk), Some(pk), Some(sig)) => (spk, pk, sig),
            _ => {
                return Err(ApiProblem::bad_request(
                    "script_public_key, public_key and signature are required to create an item on a mempool node",
                ))
            }
        };

        let spk = script_public_key.clone();
        let md = metadata.clone();
        let (tx_hash, resp) = make_threaded_call(
            &mut mempool_tx,
            move |c| {
                let (tx, tx_hash) =
                    c.create_item_asset_tx(item_amount, spk, public_key, signature, genesis_hash_spec, md)?;
                let resp = c.receive_transactions(vec![tx]);
                Ok::<_, MempoolError>((tx_hash, resp))
            },
            "create_item_asset_tx",
        )
        .await
        .map_err(|err| ApiProblem::internal(err.to_string()))? // threaded-call error
        .map_err(|err| ApiProblem::internal(err.to_string()))?; // inner MempoolError

        if !resp.success {
            return Err(ApiProblem::internal(resp.reason));
        }

        let asset = ApiAsset::from(&Asset::Item(ItemAsset::new(item_amount, Some(tx_hash.clone()), metadata)));

        return Ok((
            StatusCode::CREATED,
            Json(CreateItemResponse {
                asset,
                to_address: script_public_key,
                tx_hash,
            }),
        )
            .into_response());
    }

    if state.user_calls_tx.is_some() {
        let node = crate::v1::wallet::wallet_node(&state);
        let request = UserRequest::UserApi(UserApiRequest::SendCreateItemRequest {
            item_amount,
            genesis_hash_spec,
            metadata,
        });

        node.inject_next_event(node.local_address(), request)
            .map_err(|err| ApiProblem::internal(err.to_string()))?;

        return Ok((StatusCode::ACCEPTED, Json(CreateItemAcceptedResponse { item_amount })).into_response());
    }

    Err(ApiProblem::internal("this node cannot create item assets"))
}
