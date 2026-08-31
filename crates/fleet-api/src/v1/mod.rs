//! `/v1` route handlers and per-node routers.

pub mod blockchain;
pub mod blocks;
pub mod debug;

use axum::routing::{get, post};
use axum::{middleware, Router};
use utoipa::OpenApi as _;
use utoipa_swagger_ui::SwaggerUi;

use crate::api_key::require_api_key;
use crate::openapi::ApiDoc;
use crate::state::ApiState;

/// Router for a mempool node: `/v1/debug` (PR1 slice; more mempool resources land in
/// later phases).
pub fn mempool_router(state: ApiState) -> Router {
    build_router(state, false)
}

/// Router for a storage node: `/v1/debug` + `/v1/blocks/latest`.
pub fn storage_router(state: ApiState) -> Router {
    build_router(state, true)
}

/// Router for a miner node: `/v1/debug` (PR1 slice).
pub fn miner_router(state: ApiState) -> Router {
    build_router(state, false)
}

/// Router for a user node: `/v1/debug` (PR1 slice).
pub fn user_router(state: ApiState) -> Router {
    build_router(state, false)
}

/// Router for a pre-launch node: `/v1/debug` (PR1 slice).
pub fn pre_launch_router(state: ApiState) -> Router {
    build_router(state, false)
}

/// Shared router assembly: mounts the resources for this node, Swagger UI +
/// `/v1/openapi.json`, and the api-key layer.
fn build_router(mut state: ApiState, with_blocks: bool) -> Router {
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
    use fleet_core::configurations::DbMode;
    use fleet_core::db_utils::new_db;
    use fleet_core::interfaces::NodeType;
    use fleet_core::utils::{ApiKeys, RoutesPoWInfo};
    use fleet_core::Node;
    use http_body_util::BodyExt;
    use serde_json::Value;
    use tower::ServiceExt;

    use super::*;
    use crate::v1::debug::DebugData;

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
}
