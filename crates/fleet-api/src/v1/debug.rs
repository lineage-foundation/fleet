//! `GET /v1/debug` — node type, peer list, and route metadata for this node.
//!
//! Mirrors the legacy `debug_data` handler: reads `node.get_node_type()` /
//! `node.get_peer_list()` directly (no threaded call), folding in an optional
//! auxiliary node's data (e.g. a Miner's embedded User node) the same way.

use std::collections::BTreeMap;

use axum::extract::State;
use axum::Json;
use fleet_core::interfaces::node_type_as_str;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::state::ApiState;

/// A connected peer, as reported by `Node::get_peer_list` (mempool/storage peers only).
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct PeerInfo {
    /// The peer's socket address, as a string.
    pub address: String,
    /// The peer's node type (`Mempool` or `Storage`).
    pub node_type: String,
}

/// Debug/introspection payload for a node: type, peers, mounted routes, and PoW
/// difficulty per route. Mirrors the legacy `debug_data` handler's `DebugData`.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct DebugData {
    /// This node's type; `"{type}/{aux_type}"` when an auxiliary node is present.
    pub node_type: String,
    /// The `/v1` paths mounted on this node's router.
    pub node_api: Vec<String>,
    /// Connected peers for this node (and the auxiliary node, if present).
    pub node_peers: Vec<PeerInfo>,
    /// Per-route PoW difficulty, kept for parity with the legacy payload.
    pub routes_pow: BTreeMap<String, usize>,
}

/// Get node type, peer list, and route metadata for this node.
#[utoipa::path(
    get,
    path = "/v1/debug",
    tag = "debug",
    responses(
        (status = 200, description = "Debug data for this node", body = DebugData),
    ),
)]
pub async fn get_debug(State(state): State<ApiState>) -> Json<DebugData> {
    let node_type = node_type_as_str(state.node.get_node_type());
    let node_peers = state.node.get_peer_list().await;

    let (node_type, node_peers) = match &state.aux_node {
        Some(aux) => {
            let aux_type = node_type_as_str(aux.get_node_type());
            let aux_peers = aux.get_peer_list().await;
            (
                format!("{node_type}/{aux_type}"),
                [node_peers, aux_peers].concat(),
            )
        }
        None => (node_type.to_owned(), node_peers),
    };

    let node_peers = node_peers
        .into_iter()
        .map(|(address, _socket_addr, node_type)| PeerInfo { address, node_type })
        .collect();
    let routes_pow = state
        .routes_pow
        .lock()
        .expect("routes_pow mutex poisoned")
        .clone();

    Json(DebugData {
        node_type,
        node_api: state.mounted_routes.clone(),
        node_peers,
        routes_pow,
    })
}
