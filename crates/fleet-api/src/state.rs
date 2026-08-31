//! Shared axum handler state (node components, DB handle, API keys).

use std::sync::{Arc, Mutex};

use fleet_core::db_utils::SimpleDb;
use fleet_core::interfaces::{MempoolApi, UserApi};
use fleet_core::threaded_call::ThreadedCallSender;
use fleet_core::utils::{ApiKeys, RoutesPoWInfo};
use fleet_core::Node;

/// Shared state handed to every `/v1` handler via axum's `State` extractor.
///
/// Not every field applies to every node type: mempool carries a mempool threaded-call
/// sender, storage/user carry a DB handle, user (and a miner's embedded user node)
/// carry a user threaded-call sender, and a miner additionally carries an auxiliary
/// `Node` folded into the `/v1/debug` payload. Fields that don't apply to a given node
/// are left at their default (`None`/empty) — use the per-node constructors below
/// rather than building this directly.
#[derive(Clone)]
pub struct ApiState {
    /// This node's comms handle, used for `/v1/debug` (node type + peer list).
    pub node: Node,
    /// An auxiliary node (e.g. a Miner's embedded User node) folded into `/v1/debug`.
    pub aux_node: Option<Node>,
    /// The node's blockchain DB, where applicable (storage, user).
    pub db: Option<Arc<Mutex<SimpleDb>>>,
    /// Per-route required API keys (`route path -> allowed keys`); a route with no
    /// entry here is open.
    pub api_keys: ApiKeys,
    /// Per-route PoW difficulty, kept for `/v1/debug` payload parity with the legacy
    /// `debug_data` handler.
    pub routes_pow: RoutesPoWInfo,
    /// Threaded-call sender into a Mempool node, where applicable.
    pub mempool_calls_tx: Option<ThreadedCallSender<dyn MempoolApi>>,
    /// Threaded-call sender into a User node — used directly by User nodes, or via a
    /// Miner's embedded User node — where applicable.
    pub user_calls_tx: Option<ThreadedCallSender<dyn UserApi>>,
    /// Paths mounted on this node's router; echoed back in `/v1/debug` for parity with
    /// the legacy `DbgPaths` payload. Set by the `*_router` builders in `v1::mod`.
    pub mounted_routes: Vec<String>,
}

impl ApiState {
    fn bare(node: Node, api_keys: ApiKeys, routes_pow: RoutesPoWInfo) -> Self {
        Self {
            node,
            aux_node: None,
            db: None,
            api_keys,
            routes_pow,
            mempool_calls_tx: None,
            user_calls_tx: None,
            mounted_routes: Vec::new(),
        }
    }

    /// State for a mempool node: no DB, a mempool threaded-call sender.
    pub fn mempool(
        node: Node,
        mempool_calls_tx: ThreadedCallSender<dyn MempoolApi>,
        api_keys: ApiKeys,
        routes_pow: RoutesPoWInfo,
    ) -> Self {
        Self {
            mempool_calls_tx: Some(mempool_calls_tx),
            ..Self::bare(node, api_keys, routes_pow)
        }
    }

    /// State for a storage node: a DB handle, no threaded-call sender.
    pub fn storage(
        node: Node,
        db: Arc<Mutex<SimpleDb>>,
        api_keys: ApiKeys,
        routes_pow: RoutesPoWInfo,
    ) -> Self {
        Self {
            db: Some(db),
            ..Self::bare(node, api_keys, routes_pow)
        }
    }

    /// State for a user node: a DB handle and a user threaded-call sender.
    pub fn user(
        node: Node,
        db: Arc<Mutex<SimpleDb>>,
        user_calls_tx: ThreadedCallSender<dyn UserApi>,
        api_keys: ApiKeys,
        routes_pow: RoutesPoWInfo,
    ) -> Self {
        Self {
            db: Some(db),
            user_calls_tx: Some(user_calls_tx),
            ..Self::bare(node, api_keys, routes_pow)
        }
    }

    /// State for a miner node: the miner's own `Node` plus its embedded user node's
    /// `Node`/DB/threaded-call sender (mirrors `miner_node_with_user_routes`).
    pub fn miner(
        node: Node,
        aux_node: Node,
        db: Arc<Mutex<SimpleDb>>,
        user_calls_tx: ThreadedCallSender<dyn UserApi>,
        api_keys: ApiKeys,
        routes_pow: RoutesPoWInfo,
    ) -> Self {
        Self {
            aux_node: Some(aux_node),
            db: Some(db),
            user_calls_tx: Some(user_calls_tx),
            ..Self::bare(node, api_keys, routes_pow)
        }
    }

    /// State for a standalone miner node (no embedded user node): no DB, no
    /// threaded-call sender, no aux node.
    pub fn miner_solo(node: Node, api_keys: ApiKeys, routes_pow: RoutesPoWInfo) -> Self {
        Self::bare(node, api_keys, routes_pow)
    }

    /// State for a pre-launch node: no DB, no threaded-call sender.
    pub fn pre_launch(node: Node, api_keys: ApiKeys, routes_pow: RoutesPoWInfo) -> Self {
        Self::bare(node, api_keys, routes_pow)
    }
}
