//! `/v1` route handlers and per-node routers.

pub mod asset;
pub mod balances;
pub mod blockchain;
pub mod blocks;
pub mod debug;
pub mod items;
pub mod mining;
pub mod payments;
pub mod supply;
pub mod transactions;
pub mod tx_convert;
pub mod wallet;

use axum::routing::{get, post, put};
use axum::{middleware, Router};
use utoipa::OpenApi as _;
use utoipa_swagger_ui::SwaggerUi;

use crate::api_key::require_api_key;
use crate::openapi::ApiDoc;
use crate::state::ApiState;

/// Router for a mempool node: `/v1/debug` + supply/balances/transaction-status.
pub fn mempool_router(state: ApiState) -> Router {
    build_router(state, false, true, false)
}

/// Router for a storage node: `/v1/debug` + `/v1/blocks/latest` (and other block /
/// blockchain-entry resources).
pub fn storage_router(state: ApiState) -> Router {
    build_router(state, true, false, false)
}

/// Router for a miner node: `/v1/debug` + wallet/keypairs/outgoing-txs (when it carries
/// a wallet) + current mining block (when it mines).
pub fn miner_router(state: ApiState) -> Router {
    build_router(state, false, false, false)
}

/// Router for a user node: `/v1/debug` + wallet/keypairs/outgoing-txs (it always
/// carries a wallet) + stateless transaction serialize/deserialize.
pub fn user_router(state: ApiState) -> Router {
    build_router(state, false, false, true)
}

/// Router for a pre-launch node: `/v1/debug` (PR1 slice).
pub fn pre_launch_router(state: ApiState) -> Router {
    build_router(state, false, false, false)
}

/// Shared router assembly: mounts the resources for this node, Swagger UI +
/// `/v1/openapi.json`, and the api-key layer.
fn build_router(mut state: ApiState, with_blocks: bool, with_mempool: bool, is_user: bool) -> Router {
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
            )
            .route("/v1/transactions", post(transactions::post_create_transactions));
        mounted.push("v1/supply".to_owned());
        mounted.push("v1/balances".to_owned());
        mounted.push("v1/balances/query".to_owned());
        mounted.push("v1/transactions/status".to_owned());
        mounted.push("v1/transactions/status:query".to_owned());
        mounted.push("v1/transactions".to_owned());
    }

    if state.wallet_db.is_some() {
        router = router
            .route(
                "/v1/wallet/keypairs",
                get(wallet::get_keypairs).post(wallet::post_import_keypairs),
            )
            .route("/v1/wallet", get(wallet::get_wallet_info))
            .route("/v1/wallet/addresses", post(wallet::post_new_address))
            .route("/v1/wallet/passphrase", put(wallet::put_passphrase))
            .route(
                "/v1/wallet/running-total:refresh",
                post(wallet::post_running_total_refresh),
            )
            .route("/v1/transactions/outgoing", get(transactions::get_outgoing_txs));
        mounted.push("v1/wallet".to_owned());
        mounted.push("v1/wallet/keypairs".to_owned());
        mounted.push("v1/wallet/addresses".to_owned());
        mounted.push("v1/wallet/passphrase".to_owned());
        mounted.push("v1/wallet/running-total:refresh".to_owned());
        mounted.push("v1/transactions/outgoing".to_owned());
    }

    if state.current_block.is_some() {
        router = router.route("/v1/mining/current-block", get(mining::get_current_block));
        mounted.push("v1/mining/current-block".to_owned());
    }

    if is_user {
        router = router
            .route(
                "/v1/transactions:serialize",
                post(transactions::post_serialize_transactions),
            )
            .route(
                "/v1/transactions:deserialize",
                post(transactions::post_deserialize_transactions),
            );
        mounted.push("v1/transactions:serialize".to_owned());
        mounted.push("v1/transactions:deserialize".to_owned());
    }

    if state.mempool_calls_tx.is_some() || state.user_calls_tx.is_some() {
        router = router.route("/v1/items", post(items::post_create_item));
        mounted.push("v1/items".to_owned());
    }

    if state.user_calls_tx.is_some() {
        router = router.route("/v1/payments", post(payments::post_payment));
        mounted.push("v1/payments".to_owned());
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
    use fleet_core::interfaces::{
        CurrentBlockWithMutex, DruidPool, MempoolApi, MempoolError, NodeType, PaymentResponse, Response, TxStatus,
        TxStatusType, UserApi,
    };
    use fleet_core::threaded_call::ThreadedCallChannel;
    use fleet_core::tracked_utxo::TrackedUtxoSet;
    use fleet_core::utils::{ApiKeys, RoutesPoWInfo};
    use fleet_core::Node;
    use fleet_wallet::{AddressStore, WalletDb};
    use http_body_util::BodyExt;
    use serde_json::Value;
    use tower::ServiceExt;
    use tw_chain::crypto::sign_ed25519 as sign;
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
            Ok((
                Transaction {
                    inputs: vec![],
                    outputs: vec![],
                    version: 1,
                    fees: vec![],
                    druid_info: None,
                },
                "test_item_tx_hash".to_owned(),
            ))
        }
    }

    /// Minimal `UserApi` test double: `make_payment` returns a canned success
    /// response, exercising the address-payment success path in
    /// `v1::payments::post_payment`.
    #[derive(Default)]
    struct TestUser;

    impl UserApi for TestUser {
        fn make_payment(
            &mut self,
            _address: String,
            _amount: TokenAmount,
            _locktime: Option<u64>,
        ) -> PaymentResponse {
            PaymentResponse {
                success: true,
                reason: "ok".to_owned(),
                tx_hash: "test_payment_tx_hash".to_owned(),
                tx: None,
            }
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

    fn empty_wallet_db() -> WalletDb {
        WalletDb::new(DbMode::InMemory, None, None, None).expect("empty wallet db")
    }

    fn empty_current_block() -> CurrentBlockWithMutex {
        Arc::new(tokio::sync::Mutex::new(None))
    }

    /// A solo-miner `ApiState`: no embedded user node/DB/threaded-call sender, but it
    /// carries both a wallet DB and a current-block mutex — enough to exercise the
    /// wallet/keypairs/outgoing-txs and mining/current-block resources without needing
    /// a `UserApi` test double.
    async fn miner_solo_state() -> ApiState {
        let node = test_node(NodeType::Miner).await;
        ApiState::miner_solo(
            node,
            api_keys(vec![]),
            empty_routes_pow(),
            empty_wallet_db(),
            empty_current_block(),
        )
    }

    /// A user `ApiState`: DB handle + `TestUser` threaded-call sender + wallet DB,
    /// answering `UserApi` calls from a spawned background task for the lifetime of
    /// the test — enough to exercise `user_router`'s full route set.
    async fn user_state() -> ApiState {
        let node = test_node(NodeType::User).await;
        let db = Arc::new(Mutex::new(new_db(
            DbMode::InMemory,
            &fleet_storage::DB_SPEC,
            None,
            None,
        )));
        let ThreadedCallChannel { tx, mut rx } = ThreadedCallChannel::<dyn UserApi>::default();

        tokio::spawn(async move {
            let mut user = TestUser;
            while let Some(f) = rx.recv().await {
                f(&mut user);
            }
        });

        ApiState::user(node, db, tx, api_keys(vec![]), empty_routes_pow(), empty_wallet_db())
    }

    /// A miner-with-embedded-user `ApiState` (mirrors `miner_node_with_user_routes`):
    /// same wallet DB / current-block capabilities as `miner_solo_state`, plus an
    /// aux user node and a `TestUser` threaded-call sender. The embedded user node
    /// (its `user_calls_tx`) adds `/v1/items` over a solo miner; every other route is
    /// keyed off `wallet_db`/`current_block`, so the two share the rest of their set.
    async fn miner_with_user_state() -> ApiState {
        let node = test_node(NodeType::Miner).await;
        let aux_node = test_node(NodeType::User).await;
        let db = Arc::new(Mutex::new(new_db(
            DbMode::InMemory,
            &fleet_storage::DB_SPEC,
            None,
            None,
        )));
        let ThreadedCallChannel { tx, mut rx } = ThreadedCallChannel::<dyn UserApi>::default();

        tokio::spawn(async move {
            let mut user = TestUser;
            while let Some(f) = rx.recv().await {
                f(&mut user);
            }
        });

        ApiState::miner(
            node,
            aux_node,
            db,
            tx,
            api_keys(vec![]),
            empty_routes_pow(),
            empty_wallet_db(),
            empty_current_block(),
        )
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
    async fn post_create_transactions_returns_201_with_empty_map_for_empty_list() {
        let app = mempool_router(mempool_state(TestMempool::default()).await);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/transactions")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"transactions":[]}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::CREATED);
        let body = body_json(response).await;
        assert_eq!(body["transactions"], serde_json::json!({}));
    }

    #[tokio::test]
    async fn post_create_transactions_renders_outputs_with_the_typed_asset_shape() {
        let app = mempool_router(mempool_state(TestMempool::default()).await);

        // One output paying 42 tokens to "some_address".
        let body = serde_json::json!({
            "transactions": [{
                "inputs": [],
                "outputs": [{
                    "value": {"Token": 42},
                    "locktime": 0,
                    "script_public_key": "some_address"
                }],
                "version": 1,
                "fees": null,
                "druid_info": null
            }]
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/transactions")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::CREATED);
        let body = body_json(response).await;
        // Keyed by (derived) tx hash; assert the single entry's typed shape.
        let entry = body["transactions"]
            .as_object()
            .expect("transactions map")
            .values()
            .next()
            .expect("one output summary");
        assert_eq!(entry["address"], "some_address");
        assert_eq!(entry["asset"], serde_json::json!({"kind": "token", "amount": 42}));
    }

    #[tokio::test]
    async fn post_create_transactions_returns_400_problem_json_for_malformed_tx() {
        let app = mempool_router(mempool_state(TestMempool::default()).await);

        let body = serde_json::json!({
            "transactions": [{
                "inputs": [{
                    "previous_out": null,
                    "script_signature": {"stack": []},
                }],
                "outputs": [],
                "version": 1,
                "fees": null,
                "druid_info": null,
            }],
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/transactions")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            "application/problem+json"
        );

        let problem = body_json(response).await;
        assert_eq!(problem["status"], 400);
        assert!(problem["detail"].is_string());
    }

    /// A minimal, valid `CreateTransaction` JSON body: no inputs, a single token
    /// output.
    fn minimal_create_transaction_json() -> Value {
        serde_json::json!({
            "inputs": [],
            "outputs": [{
                "value": {"Token": 42},
                "locktime": 0,
                "script_public_key": "some_address",
            }],
            "version": 1,
            "fees": null,
            "druid_info": null,
        })
    }

    #[tokio::test]
    async fn post_serialize_transactions_returns_200_with_hex_and_hash() {
        let app = user_router(user_state().await);

        let body = serde_json::json!({ "transactions": [minimal_create_transaction_json()] });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/transactions:serialize")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        let serialized = &body["transactions"][0];
        assert!(
            serialized["txn_hex"].as_str().is_some_and(|s| !s.is_empty()),
            "txn_hex should be non-empty hex: {body:?}"
        );
        assert!(
            serialized["txn_hash_hex"].as_str().is_some_and(|s| !s.is_empty()),
            "txn_hash_hex should be non-empty: {body:?}"
        );
    }

    #[tokio::test]
    async fn post_serialize_then_deserialize_transactions_round_trips() {
        let app = user_router(user_state().await);

        let create_body = serde_json::json!({ "transactions": [minimal_create_transaction_json()] });

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/transactions:serialize")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(create_body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let serialize_body = body_json(response).await;
        let txn_hex = serialize_body["transactions"][0]["txn_hex"]
            .as_str()
            .expect("txn_hex is a string")
            .to_owned();

        let deserialize_body = serde_json::json!({ "transactions": [txn_hex] });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/transactions:deserialize")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(deserialize_body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        let round_tripped = &body["transactions"][0];
        assert_eq!(round_tripped["version"], 1);
        assert_eq!(round_tripped["outputs"][0]["value"], serde_json::json!({"Token": 42}));
    }

    #[tokio::test]
    async fn post_deserialize_transactions_returns_400_problem_json_for_bad_hex() {
        let app = user_router(user_state().await);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/transactions:deserialize")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"transactions":["not-hex"]}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            "application/problem+json"
        );

        let problem = body_json(response).await;
        assert_eq!(problem["status"], 400);
        assert!(problem["detail"].is_string());
    }

    #[tokio::test]
    async fn get_wallet_info_returns_200_with_zeroed_totals_for_empty_wallet() {
        let app = miner_router(miner_solo_state().await);

        let response = app
            .oneshot(Request::builder().uri("/v1/wallet").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(body["running_total_tokens"], 0);
        assert_eq!(body["locked_total_tokens"], 0);
        assert_eq!(body["available_total_tokens"], 0);
        assert_eq!(body["item_total"], serde_json::json!({}));
        assert_eq!(body["addresses"], serde_json::json!({}));
    }

    #[tokio::test]
    async fn get_wallet_info_accepts_page_and_spent_query_params() {
        let app = miner_router(miner_solo_state().await);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/v1/wallet?page=0&spent=true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(body["addresses"], serde_json::json!({}));
    }

    #[tokio::test]
    async fn get_keypairs_returns_200_with_empty_addresses_for_empty_wallet() {
        let app = miner_router(miner_solo_state().await);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/v1/wallet/keypairs")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(body["addresses"], serde_json::json!({}));
    }

    #[tokio::test]
    async fn post_new_address_returns_201_with_a_new_address() {
        let app = miner_router(miner_solo_state().await);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/wallet/addresses")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::CREATED);
        let body = body_json(response).await;
        let address = body["address"].as_str().expect("address is a string");
        assert!(!address.is_empty());
    }

    #[tokio::test]
    async fn put_passphrase_returns_400_problem_json_for_blank_new_passphrase() {
        let app = miner_router(miner_solo_state().await);

        let response = app
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/v1/wallet/passphrase")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"old_passphrase":"","new_passphrase":""}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            "application/problem+json"
        );

        let problem = body_json(response).await;
        assert_eq!(problem["status"], 400);
        assert!(problem["detail"].is_string());
    }

    #[tokio::test]
    async fn put_passphrase_returns_204_when_old_passphrase_matches() {
        let app = miner_router(miner_solo_state().await);

        let response = app
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/v1/wallet/passphrase")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"old_passphrase":"","new_passphrase":"hunter2"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn post_import_keypairs_returns_201_with_empty_imported_for_empty_body() {
        let app = miner_router(miner_solo_state().await);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/wallet/keypairs")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"addresses":{}}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::CREATED);
        let body = body_json(response).await;
        assert_eq!(body["imported"], serde_json::json!([]));
    }

    #[tokio::test]
    async fn post_import_keypairs_saves_nothing_when_a_later_entry_has_bad_hex() {
        // `state` shares its `WalletDb` (an `Arc<Mutex<..>>` under the hood) across
        // clones, so the router built for the follow-up GET below sees whatever the
        // failing POST actually persisted.
        let state = miner_solo_state().await;

        let (public_key, secret_key) = sign::gen_keypair();
        let good_store = AddressStore {
            public_key,
            secret_key,
            address_version: None,
        };
        let good_hex: fleet_wallet::AddressStoreHex = good_store.into();

        // Key names are chosen so "aGoodAddr" sorts before "zBadAddr" in the
        // request body's BTreeMap: under the old interleaved
        // convert-then-save-per-entry loop, the good address would already have
        // been saved by the time the bad hex was hit and the handler bailed with
        // 400.
        let body = serde_json::json!({
            "addresses": {
                "aGoodAddr": good_hex,
                "zBadAddr": {
                    "public_key": "not-hex",
                    "secret_key": "not-hex",
                    "address_version": null,
                },
            },
        });

        let app = miner_router(state.clone());
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/wallet/keypairs")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            "application/problem+json"
        );

        // No partial write: even though "aGoodAddr" sorts first and is valid, the
        // whole import must have been rejected before anything was saved.
        let app = miner_router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/v1/wallet/keypairs")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(body["addresses"], serde_json::json!({}));
    }

    #[tokio::test]
    async fn post_running_total_refresh_returns_400_when_no_addresses_given() {
        let app = user_router(user_state().await);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/wallet/running-total:refresh")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"all":false,"addresses":[]}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            "application/problem+json"
        );
        let problem = body_json(response).await;
        assert_eq!(problem["status"], 400);
    }

    #[tokio::test]
    async fn post_running_total_refresh_returns_400_when_all_true_and_wallet_is_empty() {
        let app = user_router(user_state().await);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/wallet/running-total:refresh")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"all":true,"addresses":[]}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let problem = body_json(response).await;
        assert_eq!(problem["status"], 400);
    }

    #[tokio::test]
    async fn post_running_total_refresh_returns_202_with_explicit_addresses() {
        let app = user_router(user_state().await);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/wallet/running-total:refresh")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"all":false,"addresses":["abc"]}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::ACCEPTED);
    }

    #[tokio::test]
    async fn post_running_total_refresh_routes_through_the_embedded_user_node_for_a_paired_miner() {
        // A miner paired with an embedded user node must refresh via that user node
        // (aux_node) exactly as legacy `miner_node_with_user_routes` did, not via the
        // miner node. The event injection succeeds either way on a test node, so this
        // guards that the aux-node path is wired and doesn't error.
        let app = miner_router(miner_with_user_state().await);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/wallet/running-total:refresh")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"all":false,"addresses":["abc"]}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::ACCEPTED);
    }

    #[tokio::test]
    async fn get_outgoing_txs_returns_200_empty_array_for_empty_wallet() {
        let app = miner_router(miner_solo_state().await);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/v1/transactions/outgoing")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(body["transactions"], serde_json::json!([]));
    }

    #[tokio::test]
    async fn get_current_block_returns_200_null_when_none_received() {
        let app = miner_router(miner_solo_state().await);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/v1/mining/current-block")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(body["block"], Value::Null);
    }

    #[tokio::test]
    async fn wallet_and_mining_resources_are_not_mounted_on_non_wallet_non_mining_routers() {
        for app in [
            storage_router(empty_storage_state().await),
            mempool_router(mempool_state(TestMempool::default()).await),
        ] {
            for (method, uri) in [
                ("GET", "/v1/wallet"),
                ("GET", "/v1/wallet/keypairs"),
                ("POST", "/v1/wallet/keypairs"),
                ("POST", "/v1/wallet/addresses"),
                ("PUT", "/v1/wallet/passphrase"),
                ("POST", "/v1/wallet/running-total:refresh"),
                ("GET", "/v1/transactions/outgoing"),
                ("GET", "/v1/mining/current-block"),
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
    }

    #[tokio::test]
    async fn mempool_only_resources_are_not_mounted_on_non_mempool_routers() {
        for app in [
            storage_router(empty_storage_state().await),
            user_router(user_state().await),
        ] {
            for (method, uri) in [
                ("GET", "/v1/supply"),
                ("GET", "/v1/balances"),
                ("POST", "/v1/balances/query"),
                ("GET", "/v1/transactions/status"),
                ("POST", "/v1/transactions/status:query"),
                ("POST", "/v1/transactions"),
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
    }

    #[tokio::test]
    async fn user_only_serialize_deserialize_are_not_mounted_on_non_user_routers() {
        for app in [
            mempool_router(mempool_state(TestMempool::default()).await),
            storage_router(empty_storage_state().await),
            miner_router(miner_solo_state().await),
        ] {
            for (method, uri) in [
                ("POST", "/v1/transactions:serialize"),
                ("POST", "/v1/transactions:deserialize"),
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

    #[tokio::test]
    async fn miner_with_user_router_mounts_the_same_routes_as_miner_solo_plus_items() {
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

        let mut solo = reported_routes(miner_router(miner_solo_state().await)).await;
        let with_user = reported_routes(miner_router(miner_with_user_state().await)).await;

        // Every route mounting is keyed off wallet_db/current_block, except
        // /v1/items (mempool_calls_tx/user_calls_tx) and /v1/payments
        // (user_calls_tx): a miner paired with an embedded user node (user_calls_tx)
        // can create items and make payments through it, a solo miner (neither
        // sender) cannot.
        solo.push("v1/items".to_owned());
        solo.push("v1/payments".to_owned());
        assert_eq!(
            solo, with_user,
            "an embedded user node (aux_node/user_calls_tx) should add exactly /v1/items \
             and /v1/payments to the routes miner_router mounts"
        );
    }

    /// Cross-checks `ApiDoc` (see `openapi.rs`) against what's actually mounted.
    ///
    /// Each router reports its own mounted paths on `GET /v1/debug` (`node_api`,
    /// built alongside the `.route()` calls in `build_router`). This test hits
    /// `/v1/debug` on every one of the five node router builders (`pre_launch`,
    /// `storage`, `mempool`, `miner`, `user`), unions the reported paths, and
    /// asserts that's exactly the path set `ApiDoc` documents: nothing documented
    /// but unmounted, nothing mounted but undocumented. `miner_router` is exercised
    /// via a solo-miner state; `miner_with_user_router_mounts_the_same_routes_as_miner_solo_plus_items`
    /// below confirms the with-embedded-user variant only adds `/v1/items` beyond
    /// that — already covered here via the mempool/user unions — so it isn't unioned
    /// in separately here.
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
        let miner = reported_routes(miner_router(miner_solo_state().await)).await;
        let user = reported_routes(user_router(user_state().await)).await;

        let mut mounted: Vec<String> = pre_launch
            .into_iter()
            .chain(storage)
            .chain(mempool)
            .chain(miner)
            .chain(user)
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

    #[tokio::test]
    async fn post_create_item_returns_201_with_typed_asset_on_mempool_node() {
        let app = mempool_router(mempool_state(TestMempool::default()).await);

        let body = serde_json::json!({
            "item_amount": 5,
            "genesis_hash_spec": "Create",
            "metadata": null,
            "script_public_key": "some_address",
            "public_key": "some_public_key",
            "signature": "some_signature",
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/items")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::CREATED);
        let body = body_json(response).await;
        assert_eq!(
            body["asset"],
            serde_json::json!({"kind": "item", "amount": 5, "genesis_hash": "test_item_tx_hash", "metadata": null})
        );
        assert_eq!(body["to_address"], "some_address");
        assert!(body["tx_hash"].as_str().is_some_and(|s| !s.is_empty()));
    }

    #[tokio::test]
    async fn post_create_item_returns_400_problem_json_when_signed_fields_are_missing_on_mempool_node() {
        let app = mempool_router(mempool_state(TestMempool::default()).await);

        let body = serde_json::json!({
            "item_amount": 5,
            "genesis_hash_spec": "Create",
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/items")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            "application/problem+json"
        );
        let problem = body_json(response).await;
        assert_eq!(problem["status"], 400);
        assert!(problem["detail"].is_string());
    }

    #[tokio::test]
    async fn post_create_item_returns_202_on_user_node() {
        let app = user_router(user_state().await);

        let body = serde_json::json!({
            "item_amount": 7,
            "genesis_hash_spec": "Default",
            "metadata": null,
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/items")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let body = body_json(response).await;
        assert_eq!(body["item_amount"], 7);
    }

    #[tokio::test]
    async fn post_create_item_returns_202_on_miner_with_embedded_user_node() {
        let app = miner_router(miner_with_user_state().await);

        let body = serde_json::json!({
            "item_amount": 3,
            "genesis_hash_spec": "Default",
            "metadata": null,
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/items")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let body = body_json(response).await;
        assert_eq!(body["item_amount"], 3);
    }

    #[tokio::test]
    async fn post_create_item_route_is_not_mounted_where_neither_mempool_nor_user_apply() {
        for app in [
            storage_router(empty_storage_state().await),
            pre_launch_router(ApiState::pre_launch(
                test_node(NodeType::PreLaunch).await,
                api_keys(vec![]),
                empty_routes_pow(),
            )),
            miner_router(miner_solo_state().await),
        ] {
            let response = app
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/v1/items")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();

            assert_eq!(response.status(), StatusCode::NOT_FOUND);
            assert_ne!(
                response.headers().get(header::CONTENT_TYPE),
                Some(&header::HeaderValue::from_static("application/problem+json")),
                "route should be unmounted (axum default 404), not our typed 404"
            );
        }
    }

    #[tokio::test]
    async fn post_payment_address_returns_202_with_asset_and_tx_hash_on_user_node() {
        let app = user_router(user_state().await);

        let body = serde_json::json!({
            "kind": "address",
            "address": "pay_addr",
            "amount": 5,
            "passphrase": "",
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/payments")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let body = body_json(response).await;
        assert_eq!(body["to_address"], "pay_addr");
        assert_eq!(body["amount"], serde_json::json!({"kind": "token", "amount": 5}));
        assert_eq!(body["tx_hash"], "test_payment_tx_hash");
    }

    #[tokio::test]
    async fn post_payment_ip_returns_202_with_null_tx_hash_on_user_node() {
        let app = user_router(user_state().await);

        let body = serde_json::json!({
            "kind": "ip",
            "address": "127.0.0.1:12345",
            "amount": 5,
            "passphrase": "",
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/payments")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let body = body_json(response).await;
        assert_eq!(body["tx_hash"], Value::Null);
    }

    #[tokio::test]
    async fn post_payment_ip_returns_400_problem_json_for_a_non_socket_address() {
        let app = user_router(user_state().await);

        let body = serde_json::json!({
            "kind": "ip",
            "address": "not-an-addr",
            "amount": 5,
            "passphrase": "",
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/payments")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            "application/problem+json"
        );
        let problem = body_json(response).await;
        assert_eq!(problem["status"], 400);
    }

    #[tokio::test]
    async fn post_payment_address_returns_202_on_miner_with_embedded_user_node() {
        let app = miner_router(miner_with_user_state().await);

        let body = serde_json::json!({
            "kind": "address",
            "address": "pay_addr",
            "amount": 5,
            "passphrase": "",
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/payments")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let body = body_json(response).await;
        assert_eq!(body["to_address"], "pay_addr");
        assert_eq!(body["tx_hash"], "test_payment_tx_hash");
    }

    #[tokio::test]
    async fn post_payment_route_is_not_mounted_where_user_calls_tx_is_absent() {
        for app in [
            storage_router(empty_storage_state().await),
            mempool_router(mempool_state(TestMempool::default()).await),
            miner_router(miner_solo_state().await),
        ] {
            let response = app
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/v1/payments")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();

            assert_eq!(response.status(), StatusCode::NOT_FOUND);
            assert_ne!(
                response.headers().get(header::CONTENT_TYPE),
                Some(&header::HeaderValue::from_static("application/problem+json")),
                "route should be unmounted (axum default 404), not our typed 404"
            );
        }
    }
}
