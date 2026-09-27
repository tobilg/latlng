use std::fmt;

use latlng_core::ErrorKind;
use thiserror::Error;

use crate::rpc;

#[derive(Debug, Error)]
pub enum CapnpError {
    #[error("io error: {0}")]
    Io(String),
    #[error("capnp rpc error: {0}")]
    Rpc(String),
    #[error("json error: {0}")]
    Json(String),
    #[error("unauthorized")]
    Unauthorized,
}

impl From<std::io::Error> for CapnpError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error.to_string())
    }
}

impl From<capnp::Error> for CapnpError {
    fn from(error: capnp::Error) -> Self {
        Self::Rpc(error.to_string())
    }
}

impl From<serde_json::Error> for CapnpError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error.to_string())
    }
}

/// Failure of a core call that keeps its [`rpc::ErrorCode`] so handlers can
/// report it in the response. Converts into `capnp::Error` for `?`.
#[derive(Debug)]
pub(crate) struct CallError {
    pub(crate) code: rpc::ErrorCode,
    pub(crate) message: String,
}

impl CallError {
    pub(crate) fn new(code: rpc::ErrorCode, message: impl ToString) -> Self {
        Self {
            code,
            message: message.to_string(),
        }
    }
}

impl fmt::Display for CallError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl From<latlng_core::CoreError> for CallError {
    fn from(error: latlng_core::CoreError) -> Self {
        let code = match error.kind() {
            ErrorKind::NotFound => rpc::ErrorCode::NotFound,
            ErrorKind::BadRequest => rpc::ErrorCode::BadRequest,
            ErrorKind::ReadOnly => rpc::ErrorCode::ReadOnly,
            ErrorKind::Internal => rpc::ErrorCode::Internal,
        };
        Self::new(code, error)
    }
}

impl From<CallError> for capnp::Error {
    fn from(error: CallError) -> Self {
        capnp::Error::failed(error.message)
    }
}

/// Anything a handler can report in an `error` field together with a code.
pub(crate) trait ErrorCodeOf: fmt::Display {
    fn error_code(&self) -> rpc::ErrorCode;
}

impl ErrorCodeOf for CallError {
    fn error_code(&self) -> rpc::ErrorCode {
        self.code
    }
}

impl ErrorCodeOf for capnp::Error {
    fn error_code(&self) -> rpc::ErrorCode {
        rpc::ErrorCode::Internal
    }
}
