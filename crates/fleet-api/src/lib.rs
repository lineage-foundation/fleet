//! REST API for fleet nodes, served over axum with an OpenAPI/Swagger UI harness under `/v1`.

pub mod error;
pub mod openapi;
pub mod state;
pub mod v1;
