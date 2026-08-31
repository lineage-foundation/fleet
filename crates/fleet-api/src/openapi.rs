//! OpenAPI document aggregation and Swagger UI mounting.

use utoipa::openapi::security::{ApiKey, ApiKeyValue, SecurityScheme};
use utoipa::{Modify, OpenApi};

use crate::error::ApiProblem;
#[allow(unused_imports)]
use crate::v1::blocks::{__path_get_latest_block, get_latest_block, LatestBlockResponse};
#[allow(unused_imports)]
use crate::v1::debug::{__path_get_debug, get_debug, DebugData, PeerInfo};

/// Aggregates every `/v1` operation ported so far into one OpenAPI 3.1 document,
/// served at `/v1/openapi.json` with Swagger UI at `/v1/docs`.
///
/// Not every operation listed here is mounted on every node's router (e.g.
/// `/v1/blocks/latest` is storage-only); this single document describes the API
/// surface as a whole. Per-node OpenAPI subsets are a future refinement.
#[derive(OpenApi)]
#[openapi(
    paths(get_debug, get_latest_block),
    components(schemas(DebugData, PeerInfo, LatestBlockResponse, ApiProblem)),
    modifiers(&SecurityAddon),
    info(title = "fleet REST API", description = "REST API for fleet nodes, served under /v1.")
)]
pub struct ApiDoc;

struct SecurityAddon;

impl Modify for SecurityAddon {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        let components = openapi.components.get_or_insert_with(Default::default);
        components.add_security_scheme(
            "api_key",
            SecurityScheme::ApiKey(ApiKey::Header(ApiKeyValue::new("x-api-key"))),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openapi_is_valid_3_1_and_lists_exactly_the_mounted_paths() {
        let spec = ApiDoc::openapi();
        let value = serde_json::to_value(&spec).expect("openapi doc serializes");

        assert_eq!(value["openapi"], "3.1.0");

        let mut paths: Vec<&str> = value["paths"]
            .as_object()
            .expect("paths object")
            .keys()
            .map(String::as_str)
            .collect();
        paths.sort_unstable();

        assert_eq!(paths, vec!["/v1/blocks/latest", "/v1/debug"]);
    }

    #[test]
    fn openapi_declares_the_api_key_security_scheme() {
        let spec = ApiDoc::openapi();
        let components = spec.components.expect("components present");
        assert!(components.security_schemes.contains_key("api_key"));
    }
}
