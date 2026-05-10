use axum::http::HeaderMap;
use latlng_auth::{AuthAction, AuthError, AuthPrincipal, Authenticator, extract_bearer_token};

use crate::HttpError;

pub(crate) async fn authenticate_headers(
    auth: &Authenticator,
    headers: &HeaderMap,
    cached_principal: Option<&AuthPrincipal>,
) -> Result<AuthPrincipal, HttpError> {
    if let Some(principal) = cached_principal {
        return Ok(principal.clone());
    }
    let token = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(extract_bearer_token);
    auth.authenticate(token).await.map_err(|error| match error {
        AuthError::Unauthorized => HttpError::Unauthorized,
        AuthError::InvalidConfiguration(_) | AuthError::UnsupportedAlgorithm(_) => {
            HttpError::Internal(error.to_string())
        }
    })
}

pub(crate) fn cached_auth_principal(
    cached_principal: &Option<axum::Extension<AuthPrincipal>>,
) -> Option<&AuthPrincipal> {
    cached_principal
        .as_ref()
        .map(|axum::Extension(principal)| principal)
}

pub(crate) fn ensure_collection_action(
    principal: &AuthPrincipal,
    action: AuthAction,
    collection: &str,
) -> Result<(), HttpError> {
    if principal.allows(action, collection) {
        Ok(())
    } else {
        Err(HttpError::Forbidden)
    }
}

pub(crate) fn ensure_global_action(
    principal: &AuthPrincipal,
    action: AuthAction,
) -> Result<(), HttpError> {
    if principal.allows_global(action) {
        Ok(())
    } else {
        Err(HttpError::Forbidden)
    }
}

pub(crate) fn ensure_admin(principal: &AuthPrincipal) -> Result<(), HttpError> {
    ensure_global_action(principal, AuthAction::AdminAll)
}
