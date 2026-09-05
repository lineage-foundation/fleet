//! Emits the full `/v1` OpenAPI 3.1 document — every operation across all node types — to
//! stdout as pretty JSON. This is the canonical API description; it is checked in as
//! `openapi.json` at the repo root (kept current by CI) and rendered as the public API
//! reference on the website.
//!
//! Regenerate with: `cargo run -p fleet-api --bin dump_openapi > openapi.json`

use fleet_api::openapi::ApiDoc;
use utoipa::OpenApi;

fn main() {
    let spec = ApiDoc::openapi();
    let json = serde_json::to_string_pretty(&spec).expect("serialize OpenAPI document");
    println!("{json}");
}
