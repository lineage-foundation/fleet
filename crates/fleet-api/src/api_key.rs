//! `x-api-key` enforcement, applied as a router layer.
//!
//! Mirrors the legacy warp `auth_request` semantics: the request path (minus the
//! leading `/`) is looked up in [`ApiState::api_keys`](crate::state::ApiState); a route
//! with a configured key set requires a matching `x-api-key` header, a route with no
//! entry is open.

use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::error::ApiProblem;
use crate::state::ApiState;

pub async fn require_api_key(State(state): State<ApiState>, request: Request, next: Next) -> Response {
    let route_path = request.uri().path().trim_start_matches('/').to_owned();
    let needed_keys = state
        .api_keys
        .lock()
        .expect("api_keys mutex poisoned")
        .get(&route_path)
        .cloned();

    if let Some(needed_keys) = needed_keys {
        let provided = request
            .headers()
            .get("x-api-key")
            .and_then(|value| value.to_str().ok());
        let authorized = provided
            .map(|key| needed_keys.iter().any(|needed| needed == key))
            .unwrap_or(false);

        if !authorized {
            return ApiProblem::unauthorized("missing or invalid x-api-key header").into_response();
        }
    }

    next.run(request).await
}
