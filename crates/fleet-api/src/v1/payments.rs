//! `POST /v1/payments` — unified address/ip payment endpoint for a node with a user
//! threaded-call sender (user, miner with an embedded user node).
//!
//! Reuses the legacy `post_make_payment` (address: constructs and signs the payment via
//! `UserApi::make_payment`, then injects `SendNextPayment`) and `post_make_ip_payment`
//! (ip: injects `MakeIpPayment` for the node to construct and send itself) handlers,
//! merged behind one `kind`-tagged request body.

use std::net::SocketAddr;

use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use fleet_core::interfaces::{UserApiRequest, UserRequest};
use fleet_core::threaded_call::make_threaded_call;
use serde::{Deserialize, Serialize};
use tw_chain::primitives::asset::{Asset, TokenAmount};
use utoipa::ToSchema;

use super::asset::ApiAsset;
use super::wallet::wallet_node;
use crate::error::ApiProblem;
use crate::state::ApiState;

/// Which kind of payment target `PaymentRequest::address` names.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PaymentKind {
    /// `address` is a payment (public-key) address; the node constructs and signs the
    /// payment itself, then sends it.
    Address,
    /// `address` is an `ip:port` socket address; the node constructs and sends the
    /// payment directly to that peer.
    Ip,
}

/// Request body for `POST /v1/payments`.
#[derive(Debug, Deserialize, ToSchema)]
pub struct PaymentRequest {
    pub kind: PaymentKind,
    /// A payment address (kind=address) or `ip:port` socket address (kind=ip).
    pub address: String,
    /// Amount in raw token units.
    pub amount: u64,
    pub passphrase: String,
    #[serde(default)]
    pub locktime: Option<u64>,
}

/// Response body for `POST /v1/payments`.
#[derive(Debug, Serialize, ToSchema)]
pub struct PaymentAcceptedResponse {
    /// The payment target (address or ip:port, echoing the request).
    pub to_address: String,
    /// The amount paid.
    pub amount: ApiAsset,
    /// The constructed transaction hash (present for address payments, null for ip).
    pub tx_hash: Option<String>,
}

/// Make a payment, by address or by ip.
///
/// Both kinds first check the wallet passphrase. For `kind=address`, the node
/// constructs and signs the payment (`UserApi::make_payment`); a construction failure
/// (`success=false`) is reported as `422`, otherwise the constructed payment is queued
/// for sending (`SendNextPayment`) and its `tx_hash` returned. For `kind=ip`, the
/// `address` field is parsed as a socket address and the payment is sent directly to
/// that peer (`MakeIpPayment`); no `tx_hash` is available for this path.
#[utoipa::path(
    post,
    path = "/v1/payments",
    tag = "payments",
    request_body = PaymentRequest,
    responses(
        (status = 202, description = "The payment was accepted", body = PaymentAcceptedResponse),
        (status = 400, description = "kind=ip and address was not a valid ip:port socket address", body = ApiProblem, content_type = "application/problem+json"),
        (status = 401, description = "The wallet passphrase was incorrect", body = ApiProblem, content_type = "application/problem+json"),
        (status = 422, description = "kind=address and the payment could not be constructed", body = ApiProblem, content_type = "application/problem+json"),
        (status = 500, description = "This node does not expose a wallet or cannot make payments", body = ApiProblem, content_type = "application/problem+json"),
    ),
    security(("api_key" = [])),
)]
pub async fn post_payment(
    State(state): State<ApiState>,
    Json(body): Json<PaymentRequest>,
) -> Result<(StatusCode, Json<PaymentAcceptedResponse>), ApiProblem> {
    let wallet_db = state
        .wallet_db
        .clone()
        .ok_or_else(|| ApiProblem::internal("this node does not expose a wallet"))?;

    wallet_db
        .test_passphrase(body.passphrase.clone())
        .await
        .map_err(|_| ApiProblem::unauthorized("wallet passphrase incorrect"))?;

    let amount = ApiAsset::from(&Asset::Token(TokenAmount(body.amount)));

    let tx_hash = match body.kind {
        PaymentKind::Address => {
            let mut user_tx = state
                .user_calls_tx
                .clone()
                .ok_or_else(|| ApiProblem::internal("this node cannot make payments"))?;
            let address = body.address.clone();
            let amt = body.amount;
            let locktime = body.locktime;
            let resp = make_threaded_call(
                &mut user_tx,
                move |c| c.make_payment(address, TokenAmount(amt), locktime),
                "make_payment",
            )
            .await?;

            if !resp.success {
                return Err(
                    ApiProblem::new(StatusCode::UNPROCESSABLE_ENTITY, "payment could not be constructed")
                        .with_detail(resp.reason),
                );
            }

            let node = wallet_node(&state);
            node.inject_next_event(node.local_address(), UserRequest::UserApi(UserApiRequest::SendNextPayment))
                .map_err(|err| ApiProblem::internal(err.to_string()))?;

            Some(resp.tx_hash)
        }
        PaymentKind::Ip => {
            let payment_peer: SocketAddr = body
                .address
                .parse()
                .map_err(|_| ApiProblem::bad_request("address must be a valid ip:port socket address"))?;

            let node = wallet_node(&state);
            node.inject_next_event(
                node.local_address(),
                UserRequest::UserApi(UserApiRequest::MakeIpPayment {
                    payment_peer,
                    amount: TokenAmount(body.amount),
                    locktime: body.locktime,
                }),
            )
            .map_err(|err| ApiProblem::internal(err.to_string()))?;

            None
        }
    };

    Ok((
        StatusCode::ACCEPTED,
        Json(PaymentAcceptedResponse {
            to_address: body.address,
            amount,
            tx_hash,
        }),
    ))
}
