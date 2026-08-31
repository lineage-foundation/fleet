//! API error types and `application/problem+json` response mapping.

use axum::body::Body;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use fleet_core::utils::StringError;
use serde::Serialize;
use utoipa::ToSchema;

/// An RFC 9457 `application/problem+json` error body.
///
/// Replaces the legacy byte-envelope error path: the HTTP status is authoritative, and
/// this is the one typed error shape returned by every `/v1` handler.
#[derive(Debug, Serialize, ToSchema)]
pub struct ApiProblem {
    /// A URI identifying the problem type; `about:blank` when none is defined.
    #[serde(rename = "type")]
    pub r#type: String,
    /// A short, human-readable summary of the problem type.
    pub title: String,
    /// The HTTP status code, duplicated here per RFC 9457.
    pub status: u16,
    /// A human-readable explanation specific to this occurrence of the problem.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// An optional machine-readable error code.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
}

impl ApiProblem {
    pub fn new(status: StatusCode, title: impl Into<String>) -> Self {
        Self {
            r#type: "about:blank".to_owned(),
            title: title.into(),
            status: status.as_u16(),
            detail: None,
            code: None,
        }
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub fn with_code(mut self, code: impl Into<String>) -> Self {
        self.code = Some(code.into());
        self
    }

    pub fn not_found(detail: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, "Not Found").with_detail(detail)
    }

    pub fn bad_request(detail: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "Bad Request").with_detail(detail)
    }

    pub fn unauthorized(detail: impl Into<String>) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "Unauthorized").with_detail(detail)
    }

    pub fn internal(detail: impl Into<String>) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "Internal Server Error").with_detail(detail)
    }
}

/// Node/threaded-call failures surface as `500`s: the request was well-formed, the
/// node-side operation itself failed.
impl From<StringError> for ApiProblem {
    fn from(err: StringError) -> Self {
        Self::internal(err.0)
    }
}

impl IntoResponse for ApiProblem {
    fn into_response(self) -> Response {
        let status = StatusCode::from_u16(self.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        let body = serde_json::to_vec(&self).unwrap_or_default();
        Response::builder()
            .status(status)
            .header(header::CONTENT_TYPE, "application/problem+json")
            .body(Body::from(body))
            .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
    }
}
