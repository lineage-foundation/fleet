//! REST API for fleet nodes, served over axum with an OpenAPI/Swagger UI harness under `/v1`.

pub mod api_key;
pub mod error;
pub mod openapi;
pub mod state;
pub mod v1;

pub use state::ApiState;
pub use v1::{mempool_router, miner_router, pre_launch_router, storage_router, user_router};
