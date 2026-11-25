//! The one error shape the whole service uses, per docs/API.md: every
//! non-2xx HTTP response, and every failed job's `error` field, is
//! `{"error": "<machine code>", "message": "<human text>"}`.

use axum::response::{IntoResponse, Response};
use axum::Json;
use axum::{
    extract::{
        rejection::{JsonRejection, QueryRejection},
        FromRequest, FromRequestParts, Query,
    },
    http::{header, request::Parts, HeaderValue, StatusCode},
};
use serde::{de::DeserializeOwned, Serialize};

/// Query parameters whose extraction failures use the service's JSON error shape.
pub struct ApiQuery<T>(pub T);

/// JSON bodies whose extraction failures use the service's JSON error shape.
pub struct ApiJson<T>(pub T);

impl<T, S> FromRequest<S> for ApiJson<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request(req: axum::extract::Request, state: &S) -> Result<Self, Self::Rejection> {
        Json::<T>::from_request(req, state)
            .await
            .map(|Json(value)| Self(value))
            .map_err(|rejection: JsonRejection| ApiError::invalid_request(rejection.body_text()))
    }
}

impl<T, S> FromRequestParts<S> for ApiQuery<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        Query::<T>::from_request_parts(parts, state)
            .await
            .map(|Query(value)| Self(value))
            .map_err(|rejection: QueryRejection| ApiError::invalid_request(rejection.body_text()))
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct ErrorBody {
    pub error: String,
    pub message: String,
}

/// An HTTP-facing error: an [`ErrorBody`] plus the status code it maps to.
/// Job failures store just the [`ErrorBody`] (a job does not have an HTTP
/// status of its own -- it already returned 202 -- see docs/API.md), and
/// convert to a full `ApiError` only where a job's terminal state also
/// needs to answer an HTTP request (there is no such path in this
/// milestone, but keeping the two separate is what makes that safe to add
/// later without a shape change).
#[derive(Clone, Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub body: ErrorBody,
}

impl ApiError {
    pub fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        ApiError {
            status,
            body: ErrorBody {
                error: code.to_owned(),
                message: message.into(),
            },
        }
    }

    pub fn invalid_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "invalid_request", message)
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, "not_found", message)
    }

    pub fn busy(message: impl Into<String>) -> Self {
        Self::new(StatusCode::SERVICE_UNAVAILABLE, "busy", message)
    }

    /// A personal web job was interrupted by process shutdown; retry explicitly.
    pub fn server_stopping(message: impl Into<String>) -> Self {
        Self::new(StatusCode::SERVICE_UNAVAILABLE, "server_stopping", message)
    }

    pub fn detection_failed(message: impl Into<String>) -> Self {
        Self::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "detection_failed",
            message,
        )
    }

    /// Detection succeeded but at low confidence (`detect::Confidence::Low`)
    /// -- a wrong source root produces a plausible-looking wrong map
    /// (finding 7), so this is surfaced distinctly rather than indexed
    /// silently. `message` is `SourceCandidate::describe()`'s evidence
    /// text, not a generic refusal.
    pub fn detection_uncertain(message: impl Into<String>) -> Self {
        Self::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "detection_uncertain",
            message,
        )
    }

    pub fn clone_failed(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_GATEWAY, "clone_failed", message)
    }

    pub fn index_failed(message: impl Into<String>) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "index_failed", message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", message)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let busy = self.status == StatusCode::SERVICE_UNAVAILABLE && self.body.error == "busy";
        let mut response = (self.status, Json(self.body)).into_response();
        if busy {
            response
                .headers_mut()
                .insert(header::RETRY_AFTER, HeaderValue::from_static("30"));
        }
        response
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(err: anyhow::Error) -> Self {
        ApiError::internal(format!("{err:#}"))
    }
}
