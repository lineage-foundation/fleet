//! `POST /v1/donation-requests` — ask a peer to send this user node a donation.
//!
//! Reuses the legacy `post_request_donation` handler: parses `address` as a socket
//! address and injects `RequestDonation` for the node to send the request itself.
//! User-node only (not exposed on a miner paired with an embedded user node).

use std::net::SocketAddr;

use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use fleet_core::interfaces::{UserApiRequest, UserRequest};
use serde::Deserialize;
use utoipa::ToSchema;

use super::wallet::wallet_node;
use crate::error::ApiProblem;
use crate::state::ApiState;

/// Request body for `POST /v1/donation-requests`.
#[derive(Debug, Deserialize, ToSchema)]
pub struct DonationRequest {
    /// The `ip:port` socket address of the peer to request a donation from.
    pub address: String,
}

/// Ask a peer to send this user node a donation.
///
/// `address` is parsed as a socket address and a `RequestDonation` event is injected
/// for the node to send the request itself; no response payload is returned.
#[utoipa::path(
    post,
    path = "/v1/donation-requests",
    tag = "donations",
    request_body = DonationRequest,
    responses(
        (status = 202, description = "Donation request sent"),
        (status = 400, description = "address was not a valid ip:port socket address", body = ApiProblem, content_type = "application/problem+json"),
        (status = 500, description = "The donation request could not be sent", body = ApiProblem, content_type = "application/problem+json"),
    ),
    security(("api_key" = [])),
)]
pub async fn post_donation_request(
    State(state): State<ApiState>,
    Json(body): Json<DonationRequest>,
) -> Result<StatusCode, ApiProblem> {
    let paying_peer: SocketAddr = body
        .address
        .parse()
        .map_err(|_| ApiProblem::bad_request("address must be a valid ip:port socket address"))?;

    let node = wallet_node(&state);
    node.inject_next_event(
        node.local_address(),
        UserRequest::UserApi(UserApiRequest::RequestDonation { paying_peer }),
    )
    .map_err(|err| ApiProblem::internal(err.to_string()))?;

    Ok(StatusCode::ACCEPTED)
}
