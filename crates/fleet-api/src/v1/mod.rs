//! `/v1` route handlers and per-node routers.

pub mod balances;
pub mod blockchain;
pub mod blocks;
pub mod debug;
pub mod supply;
pub mod transactions;

use axum::routing::{get, post};
use axum::{middleware, Router};
use utoipa::OpenApi as _;
use utoipa_swagger_ui::SwaggerUi;

use crate::api_key::require_api_key;
use crate::openapi::ApiDoc;
use crate::state::ApiState;

/// Router for a mempool node: `/v1/debug` + supply/balances/transaction-status.
pub fn mempool_router(state: ApiState) -> Router {
    build_router(state, false, true)
}

/// Router for a storage node: `/v1/debug` + `/v1/blocks/latest` (and other block /
/// blockchain-entry resources).
pub fn storage_router(state: ApiState) -> Router {
    build_router(state, true, false)
}

/// Router for a miner node: `/v1/debug` (PR1 slice).
pub fn miner_router(state: ApiState) -> Router {
    build_router(state, false, false)
}

/// Router for a user node: `/v1/debug` (PR1 slice).
pub fn user_router(state: ApiState) -> Router {
    build_router(state, false, false)
}

/// Router for a pre-launch node: `/v1/debug` (PR1 slice).
pub fn pre_launch_router(state: ApiState) -> Router {
    build_router(state, false, false)
}

/// Shared router assembly: mounts the resources for this node, Swagger UI +
/// `/v1/openapi.json`, and the api-key layer.
fn build_router(mut state: ApiState, with_blocks: bool, with_mempool: bool) -> Router {
    let mut router = Router::new().route("/v1/debug", get(debug::get_debug));
    let mut mounted = vec!["v1/debug".to_owned()];

    if with_blocks {
        router = router
            .route("/v1/blocks/latest", get(blocks::get_latest_block))
            .route("/v1/blocks/{num}", get(blocks::get_block_by_num))
            .route("/v1/blocks", get(blocks::get_blocks_batch))
            .route("/v1/blockchain-entries/{key}", get(blockchain::get_blockchain_entry))
            .route(
                "/v1/blockchain-entries/query",
                post(blockchain::query_blockchain_entries),
            );
        mounted.push("v1/blocks/latest".to_owned());
        mounted.push("v1/blocks/{num}".to_owned());
        mounted.push("v1/blocks".to_owned());
        mounted.push("v1/blockchain-entries/{key}".to_owned());
        mounted.push("v1/blockchain-entries/query".to_owned());
    }

    if with_mempool {
        router = router
            .route("/v1/supply", get(supply::get_supply))
            .route("/v1/balances", get(balances::get_balances))
            .route("/v1/balances/query", post(balances::query_balances))
            .route(
                "/v1/transactions/status",
                get(transactions::get_transaction_status),
            )
            .route(
                "/v1/transactions/status:query",
                post(transactions::query_transaction_status),
            );
        mounted.push("v1/supply".to_owned());
        mounted.push("v1/balances".to_owned());
        mounted.push("v1/balances/query".to_owned());
        mounted.push("v1/transactions/status".to_owned());
        mounted.push("v1/transactions/status:query".to_owned());
    }
    state.mounted_routes = mounted;

    router
        .merge(SwaggerUi::new("/v1/docs").url("/v1/openapi.json", ApiDoc::openapi()))
        .layer(middleware::from_fn_with_state(state.clone(), require_api_key))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};

    use axum::body::Body;
    use axum::http::{header, Request, StatusCode};
    use fleet_core::comms_handler::TcpTlsConfig;
    use fleet_core::configurations::{DbMode, MempoolNodeSharedConfig, MinerWhitelist};
    use fleet_core::db_utils::new_db;
    use fleet_core::interfaces::{DruidPool, MempoolApi, MempoolError, NodeType, Response, TxStatus, TxStatusType};
    use fleet_core::threaded_call::ThreadedCallChannel;
    use fleet_core::tracked_utxo::TrackedUtxoSet;
    use fleet_core::utils::{ApiKeys, RoutesPoWInfo};
    use fleet_core::Node;
    use http_body_util::BodyExt;
    use serde_json::Value;
    use tower::ServiceExt;
    use tw_chain::primitives::asset::TokenAmount;
    use tw_chain::primitives::transaction::{GenesisTxHashSpec, Transaction};

    use super::*;
    use crate::v1::debug::DebugData;

    /// Minimal `MempoolApi` test double: canned responses for every method except
    /// `get_issued_supply`/`get_committed_utxo_tracked_set`/`get_transaction_status`,
    /// which return whatever's configured on the struct. Drains its threaded-call
    /// receiver in a background task, mirroring `fleet_core::threaded_call`'s own test.
    #[derive(Default)]
    struct TestMempool {
        issued_supply: TokenAmount,
        utxos: TrackedUtxoSet,
        druid_pool: DruidPool,
        tx_status: BTreeMap<String, TxStatus>,
    }

    impl MempoolApi for TestMempool {
        fn get_shared_config(&self) -> MempoolNodeSharedConfig {
            MempoolNodeSharedConfig {
                mempool_mining_event_timeout: 0,
                mempool_partition_full_size: 0,
                mempool_miner_whitelist: MinerWhitelist::default(),
            }
        }

        fn pause_nodes(&mut self, _b_num: u64) -> Response {
            Response {
                success: true,
                reason: "paused".to_owned(),
            }
        }

        fn resume_nodes(&mut self) -> Response {
            Response {
                success: true,
                reason: "resumed".to_owned(),
            }
        }

        fn send_shared_config(&mut self, _shared_config: MempoolNodeSharedConfig) -> Response {
            Response {
                success: true,
                reason: "config shared".to_owned(),
            }
        }

        fn get_committed_utxo_tracked_set(&self) -> &TrackedUtxoSet {
            &self.utxos
        }

        fn get_issued_supply(&self) -> TokenAmount {
            self.issued_supply
        }

        fn get_pending_druid_pool(&self) -> &DruidPool {
            &self.druid_pool
        }

        fn get_transaction_status(&self, _tx_hashes: Vec<String>) -> BTreeMap<String, TxStatus> {
            self.tx_status.clone()
        }

        fn receive_transactions(&mut self, _transactions: Vec<Transaction>) -> Response {
            Response {
                success: true,
                reason: "received".to_owned(),
            }
        }

        fn create_item_asset_tx(
            &mut self,
            _item_amount: u64,
            _script_public_key: String,
            _public_key: String,
            _signature: String,
            _genesis_hash_spec: GenesisTxHashSpec,
            _metadata: Option<String>,
        ) -> Result<(Transaction, String), MempoolError> {
            Err(MempoolError::ConfigError("not implemented in test mock"))
        }
    }

    /// Build a mempool `ApiState` backed by `mempool`, answering threaded calls from a
    /// spawned background task for the lifetime of the test.
    async fn mempool_state(mempool: TestMempool) -> ApiState {
        let node = test_node(NodeType::Mempool).await;
        let ThreadedCallChannel { tx, mut rx } = ThreadedCallChannel::<dyn MempoolApi>::default();

        tokio::spawn(async move {
            let mut mempool = mempool;
            while let Some(f) = rx.recv().await {
                f(&mut mempool);
            }
        });

        ApiState::mempool(node, tx, api_keys(vec![]), empty_routes_pow())
    }

    async fn test_node(node_type: NodeType) -> Node {
        let config = TcpTlsConfig::new_no_tls("127.0.0.1:0".parse().unwrap());
        Node::new(&config, 1, 1, node_type, true, false)
            .await
            .expect("test node")
    }

    fn api_keys(entries: Vec<(&str, Vec<&str>)>) -> ApiKeys {
        let map: BTreeMap<String, Vec<String>> = entries
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v.into_iter().map(str::to_owned).collect()))
            .collect();
        Arc::new(Mutex::new(map))
    }

    fn empty_routes_pow() -> RoutesPoWInfo {
        Arc::new(Mutex::new(BTreeMap::new()))
    }

    async fn body_json(response: axum::response::Response) -> Value {
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).expect("valid json body")
    }

    async fn empty_storage_state() -> ApiState {
        let node = test_node(NodeType::Storage).await;
        let db = Arc::new(Mutex::new(new_db(
            DbMode::InMemory,
            &fleet_storage::DB_SPEC,
            None,
            None,
        )));
        ApiState::storage(node, db, api_keys(vec![]), empty_routes_pow())
    }

    #[tokio::test]
    async fn get_debug_returns_200_and_typed_debug_data() {
        let node = test_node(NodeType::PreLaunch).await;
        let state = ApiState::pre_launch(node, api_keys(vec![]), empty_routes_pow());
        let app = pre_launch_router(state);

        let response = app
            .oneshot(Request::builder().uri("/v1/debug").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let data: DebugData = serde_json::from_slice(&body).expect("valid DebugData json");
        assert_eq!(data.node_type, "Prelaunch");
        assert_eq!(data.node_api, vec!["v1/debug".to_owned()]);
        assert!(data.node_peers.is_empty());
    }

    #[tokio::test]
    async fn get_latest_block_returns_404_problem_json_on_empty_db() {
        let node = test_node(NodeType::Storage).await;
        let db = Arc::new(Mutex::new(new_db(
            DbMode::InMemory,
            &fleet_storage::DB_SPEC,
            None,
            None,
        )));
        let state = ApiState::storage(node, db, api_keys(vec![]), empty_routes_pow());
        let app = storage_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/v1/blocks/latest")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            "application/problem+json"
        );

        let problem = body_json(response).await;
        assert_eq!(problem["status"], 404);
        assert!(problem["detail"].is_string());
    }

    #[tokio::test]
    async fn get_block_by_num_returns_404_problem_json_on_empty_db() {
        let app = storage_router(empty_storage_state().await);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/v1/blocks/1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            "application/problem+json"
        );

        let problem = body_json(response).await;
        assert_eq!(problem["status"], 404);
        assert!(problem["detail"].is_string());
    }

    #[tokio::test]
    async fn get_blocks_batch_returns_200_empty_array_on_empty_db() {
        let app = storage_router(empty_storage_state().await);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/v1/blocks?num=1&num=2")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(body, Value::Array(vec![]));
    }

    #[tokio::test]
    async fn get_blockchain_entry_returns_404_problem_json_on_empty_db() {
        let app = storage_router(empty_storage_state().await);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/v1/blockchain-entries/some-key")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            "application/problem+json"
        );

        let problem = body_json(response).await;
        assert_eq!(problem["status"], 404);
        assert!(problem["detail"].is_string());
    }

    #[tokio::test]
    async fn query_blockchain_entries_returns_200_empty_array_on_empty_db() {
        let app = storage_router(empty_storage_state().await);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/blockchain-entries/query")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"keys":["some-key","other-key"]}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(body, Value::Array(vec![]));
    }

    #[tokio::test]
    async fn storage_only_resources_are_not_mounted_on_non_storage_routers() {
        let node = test_node(NodeType::PreLaunch).await;
        let state = ApiState::pre_launch(node, api_keys(vec![]), empty_routes_pow());
        let app = pre_launch_router(state);

        for (method, uri) in [
            ("GET", "/v1/blocks/1"),
            ("GET", "/v1/blocks"),
            ("GET", "/v1/blockchain-entries/some-key"),
            ("POST", "/v1/blockchain-entries/query"),
        ] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method(method)
                        .uri(uri)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();

            assert_eq!(response.status(), StatusCode::NOT_FOUND, "unexpected mount: {method} {uri}");
            assert_ne!(
                response.headers().get(header::CONTENT_TYPE),
                Some(&header::HeaderValue::from_static("application/problem+json")),
                "route should be unmounted (axum default 404), not our typed 404: {method} {uri}"
            );
        }
    }

    #[tokio::test]
    async fn get_supply_returns_200_with_total_and_issued() {
        let mempool = TestMempool {
            issued_supply: TokenAmount(42),
            ..Default::default()
        };
        let app = mempool_router(mempool_state(mempool).await);

        let response = app
            .oneshot(Request::builder().uri("/v1/supply").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(body["total"].as_u64(), Some(fleet_core::constants::TOTAL_TOKENS));
        assert_eq!(body["issued"].as_u64(), Some(42));
    }

    #[tokio::test]
    async fn get_balances_returns_200_with_typed_balance() {
        let app = mempool_router(mempool_state(TestMempool::default()).await);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/v1/balances?address=some-address")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        assert!(body["balance"].is_object(), "balance should be a JSON object: {body:?}");
    }

    #[tokio::test]
    async fn query_balances_returns_200_with_typed_balance() {
        let app = mempool_router(mempool_state(TestMempool::default()).await);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/balances/query")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"addresses":["some-address","other-address"]}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        assert!(body["balance"].is_object(), "balance should be a JSON object: {body:?}");
    }

    #[tokio::test]
    async fn get_transaction_status_returns_200_with_typed_status() {
        let mut tx_status = BTreeMap::new();
        tx_status.insert(
            "some-hash".to_owned(),
            TxStatus {
                status: TxStatusType::Pending,
                timestamp: 123,
                additional_info: "queued".to_owned(),
            },
        );
        let mempool = TestMempool {
            tx_status,
            ..Default::default()
        };
        let app = mempool_router(mempool_state(mempool).await);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/v1/transactions/status?hash=some-hash")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(body["some-hash"]["status"], "Pending");
        assert_eq!(body["some-hash"]["timestamp"], 123);
        assert_eq!(body["some-hash"]["additional_info"], "queued");
    }

    #[tokio::test]
    async fn query_transaction_status_returns_200_with_typed_status() {
        let mut tx_status = BTreeMap::new();
        tx_status.insert(
            "some-hash".to_owned(),
            TxStatus {
                status: TxStatusType::Confirmed,
                timestamp: 456,
                additional_info: "in block".to_owned(),
            },
        );
        let mempool = TestMempool {
            tx_status,
            ..Default::default()
        };
        let app = mempool_router(mempool_state(mempool).await);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/transactions/status:query")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"hashes":["some-hash"]}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(body["some-hash"]["status"], "Confirmed");
        assert_eq!(body["some-hash"]["timestamp"], 456);
        assert_eq!(body["some-hash"]["additional_info"], "in block");
    }

    #[tokio::test]
    async fn mempool_only_resources_are_not_mounted_on_non_mempool_routers() {
        let app = storage_router(empty_storage_state().await);

        for (method, uri) in [
            ("GET", "/v1/supply"),
            ("GET", "/v1/balances"),
            ("POST", "/v1/balances/query"),
            ("GET", "/v1/transactions/status"),
            ("POST", "/v1/transactions/status:query"),
        ] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method(method)
                        .uri(uri)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();

            assert_eq!(response.status(), StatusCode::NOT_FOUND, "unexpected mount: {method} {uri}");
            assert_ne!(
                response.headers().get(header::CONTENT_TYPE),
                Some(&header::HeaderValue::from_static("application/problem+json")),
                "route should be unmounted (axum default 404), not our typed 404: {method} {uri}"
            );
        }
    }

    #[tokio::test]
    async fn debug_endpoint_requires_configured_api_key() {
        let node = test_node(NodeType::PreLaunch).await;
        let state = ApiState::pre_launch(
            node,
            api_keys(vec![("v1/debug", vec!["secret"])]),
            empty_routes_pow(),
        );
        let app = pre_launch_router(state);

        let unauthorized = app
            .clone()
            .oneshot(Request::builder().uri("/v1/debug").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            unauthorized.headers().get(header::CONTENT_TYPE).unwrap(),
            "application/problem+json"
        );

        let authorized = app
            .oneshot(
                Request::builder()
                    .uri("/v1/debug")
                    .header("x-api-key", "secret")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(authorized.status(), StatusCode::OK);
    }

    /// Cross-checks `ApiDoc` (see `openapi.rs`) against what's actually mounted.
    ///
    /// Each router reports its own mounted paths on `GET /v1/debug` (`node_api`,
    /// built alongside the `.route()` calls in `build_router`). This test hits
    /// `/v1/debug` on every node router, unions the reported paths, and asserts
    /// that's exactly the path set `ApiDoc` documents: nothing documented but
    /// unmounted, nothing mounted but undocumented.
    ///
    /// This isn't a raw reflection of axum's internal route table (axum doesn't
    /// expose one), so it still relies on `node_api` staying in sync with the
    /// `.route()` calls a few lines above it in `build_router` — but that's the
    /// same value every node reports to callers, so drift there is itself a bug.
    #[tokio::test]
    async fn openapi_documents_exactly_the_union_of_every_router_mounted_routes() {
        async fn reported_routes(app: Router) -> Vec<String> {
            let response = app
                .oneshot(Request::builder().uri("/v1/debug").body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let data: DebugData = serde_json::from_slice(
                &response.into_body().collect().await.unwrap().to_bytes(),
            )
            .expect("valid DebugData json");
            data.node_api
        }

        let pre_launch = reported_routes(pre_launch_router(ApiState::pre_launch(
            test_node(NodeType::PreLaunch).await,
            api_keys(vec![]),
            empty_routes_pow(),
        )))
        .await;
        let storage = reported_routes(storage_router(empty_storage_state().await)).await;
        let mempool = reported_routes(mempool_router(mempool_state(TestMempool::default()).await)).await;

        let mut mounted: Vec<String> = pre_launch
            .into_iter()
            .chain(storage)
            .chain(mempool)
            .map(|route| format!("/{route}"))
            .collect();
        mounted.sort_unstable();
        mounted.dedup();

        let spec = crate::openapi::ApiDoc::openapi();
        let value = serde_json::to_value(&spec).expect("openapi doc serializes");
        let mut documented: Vec<String> = value["paths"]
            .as_object()
            .expect("paths object")
            .keys()
            .cloned()
            .collect();
        documented.sort_unstable();

        assert_eq!(
            documented, mounted,
            "ApiDoc must list exactly the union of paths mounted across all node routers"
        );
    }
}
