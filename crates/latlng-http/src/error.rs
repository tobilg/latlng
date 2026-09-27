use axum::extract::FromRequest;
use axum::extract::FromRequestParts;
use axum::extract::rejection::{JsonRejection, PathRejection, QueryRejection};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum HttpError {
    #[error("unauthorized")]
    Unauthorized,
    #[error("forbidden")]
    Forbidden,
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("unsupported media type: {0}")]
    UnsupportedMediaType(String),
    #[error("service unavailable: {0}")]
    Unavailable(String),
    #[error("internal error: {0}")]
    Internal(String),
}

impl IntoResponse for HttpError {
    fn into_response(self) -> Response {
        let status = match self {
            Self::Unauthorized => StatusCode::UNAUTHORIZED,
            Self::Forbidden => StatusCode::FORBIDDEN,
            Self::BadRequest(_) => StatusCode::BAD_REQUEST,
            Self::NotFound(_) => StatusCode::NOT_FOUND,
            Self::UnsupportedMediaType(_) => StatusCode::UNSUPPORTED_MEDIA_TYPE,
            Self::Unavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
            Self::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (
            status,
            axum::Json(serde_json::json!({ "error": self.to_string() })),
        )
            .into_response()
    }
}

impl From<JsonRejection> for HttpError {
    fn from(rejection: JsonRejection) -> Self {
        match rejection {
            JsonRejection::MissingJsonContentType(_) => {
                Self::UnsupportedMediaType(rejection.body_text())
            }
            // Syntax errors, schema mismatches and unknown fields are all the
            // caller's fault, so they share one status.
            other => Self::BadRequest(other.body_text()),
        }
    }
}

impl From<PathRejection> for HttpError {
    fn from(rejection: PathRejection) -> Self {
        Self::BadRequest(rejection.body_text())
    }
}

impl From<QueryRejection> for HttpError {
    fn from(rejection: QueryRejection) -> Self {
        Self::BadRequest(rejection.body_text())
    }
}

/// JSON extractor and response whose rejections are JSON `HttpError` bodies
/// instead of axum's plain-text defaults.
#[derive(FromRequest)]
#[from_request(via(axum::Json), rejection(HttpError))]
pub(crate) struct Json<T>(pub T);

impl<T: Serialize> IntoResponse for Json<T> {
    fn into_response(self) -> Response {
        axum::Json(self.0).into_response()
    }
}

/// Path extractor whose rejections are JSON `HttpError` bodies.
#[derive(FromRequestParts)]
#[from_request(via(axum::extract::Path), rejection(HttpError))]
pub(crate) struct Path<T>(pub T);

/// Query extractor whose rejections are JSON `HttpError` bodies.
#[derive(FromRequestParts)]
#[from_request(via(axum::extract::Query), rejection(HttpError))]
pub(crate) struct Query<T>(pub T);

pub(crate) fn json_error_response(status: StatusCode, error: impl ToString) -> Response {
    (
        status,
        axum::Json(serde_json::json!({ "error": error.to_string() })),
    )
        .into_response()
}
