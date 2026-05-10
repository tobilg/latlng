use thiserror::Error;

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
