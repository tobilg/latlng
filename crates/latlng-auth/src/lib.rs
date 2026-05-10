#![forbid(unsafe_code)]

use std::collections::HashSet;
use std::time::{Duration, Instant};

use glob_match::glob_match;
use jsonwebtoken::jwk::JwkSet;
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::sync::RwLock;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthConfig {
    pub bearer_token: Option<String>,
    #[serde(default)]
    pub disable_bearer_token: bool,
    pub jwt_secret: Option<String>,
    pub jwt_public_key_pem: Option<String>,
    pub jwt_issuer: Option<String>,
    pub jwt_audience: Option<String>,
    pub jwt_algorithm: Option<String>,
    #[serde(default)]
    pub jwks_url: Option<String>,
    #[serde(default)]
    pub jwks_provider_id: Option<String>,
    #[serde(default = "default_jwks_refresh_interval_seconds")]
    pub jwks_refresh_interval_seconds: u64,
    #[serde(default = "default_jwks_cache_ttl_seconds")]
    pub jwks_cache_ttl_seconds: u64,
    #[serde(default = "default_jwks_http_timeout_ms")]
    pub jwks_http_timeout_ms: u64,
    #[serde(default)]
    pub jwt_leeway_seconds: u64,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            bearer_token: None,
            disable_bearer_token: false,
            jwt_secret: None,
            jwt_public_key_pem: None,
            jwt_issuer: None,
            jwt_audience: None,
            jwt_algorithm: None,
            jwks_url: None,
            jwks_provider_id: None,
            jwks_refresh_interval_seconds: default_jwks_refresh_interval_seconds(),
            jwks_cache_ttl_seconds: default_jwks_cache_ttl_seconds(),
            jwks_http_timeout_ms: default_jwks_http_timeout_ms(),
            jwt_leeway_seconds: 0,
        }
    }
}

pub fn default_jwks_refresh_interval_seconds() -> u64 {
    300
}

pub fn default_jwks_cache_ttl_seconds() -> u64 {
    3_600
}

pub fn default_jwks_http_timeout_ms() -> u64 {
    3_000
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AuthAction {
    CollectionsList,
    CollectionsCreate,
    CollectionsDelete,
    CollectionsInspect,
    ObjectsRead,
    ObjectsWrite,
    ObjectsDelete,
    QueriesRead,
    SubscriptionsRead,
    HooksManage,
    ChannelsManage,
    MetricsRead,
    AdminAll,
}

impl AuthAction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CollectionsList => "collections:list",
            Self::CollectionsCreate => "collections:create",
            Self::CollectionsDelete => "collections:delete",
            Self::CollectionsInspect => "collections:inspect",
            Self::ObjectsRead => "objects:read",
            Self::ObjectsWrite => "objects:write",
            Self::ObjectsDelete => "objects:delete",
            Self::QueriesRead => "queries:read",
            Self::SubscriptionsRead => "subscriptions:read",
            Self::HooksManage => "hooks:manage",
            Self::ChannelsManage => "channels:manage",
            Self::MetricsRead => "metrics:read",
            Self::AdminAll => "admin:*",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "collections:list" => Some(Self::CollectionsList),
            "collections:create" => Some(Self::CollectionsCreate),
            "collections:delete" => Some(Self::CollectionsDelete),
            "collections:inspect" => Some(Self::CollectionsInspect),
            "objects:read" => Some(Self::ObjectsRead),
            "objects:write" => Some(Self::ObjectsWrite),
            "objects:delete" => Some(Self::ObjectsDelete),
            "queries:read" => Some(Self::QueriesRead),
            "subscriptions:read" => Some(Self::SubscriptionsRead),
            "hooks:manage" => Some(Self::HooksManage),
            "channels:manage" => Some(Self::ChannelsManage),
            "metrics:read" => Some(Self::MetricsRead),
            "admin:*" => Some(Self::AdminAll),
            _ => None,
        }
    }
}

const COLLECTION_VISIBILITY_ACTIONS: [AuthAction; 8] = [
    AuthAction::CollectionsInspect,
    AuthAction::ObjectsRead,
    AuthAction::QueriesRead,
    AuthAction::SubscriptionsRead,
    AuthAction::ObjectsWrite,
    AuthAction::ObjectsDelete,
    AuthAction::HooksManage,
    AuthAction::ChannelsManage,
];

#[derive(Debug, Clone)]
pub struct PermissionRule {
    patterns: Vec<String>,
    actions: HashSet<AuthAction>,
}

impl PermissionRule {
    fn matches_collection(&self, collection: &str) -> bool {
        self.patterns
            .iter()
            .any(|pattern| pattern == "*" || glob_match(pattern, collection))
    }

    fn is_global(&self) -> bool {
        self.patterns.iter().any(|pattern| pattern == "*")
    }

    fn allows(&self, action: AuthAction, collection: &str) -> bool {
        self.actions.contains(&action) && self.matches_collection(collection)
    }

    fn allows_global(&self, action: AuthAction) -> bool {
        self.actions.contains(&action) && self.is_global()
    }
}

#[derive(Debug, Clone)]
pub struct AuthPrincipal {
    pub rate_limit_key: String,
    admin: bool,
    permissions: Vec<PermissionRule>,
}

impl AuthPrincipal {
    pub fn open_access() -> Self {
        Self {
            rate_limit_key: "open:access".to_owned(),
            admin: true,
            permissions: Vec::new(),
        }
    }

    pub fn service_admin() -> Self {
        Self {
            rate_limit_key: "bearer:service".to_owned(),
            admin: true,
            permissions: Vec::new(),
        }
    }

    pub fn anonymous() -> Self {
        Self {
            rate_limit_key: "anonymous".to_owned(),
            admin: false,
            permissions: Vec::new(),
        }
    }

    fn from_claims(claims: JwtClaims, token: &str) -> Self {
        let rate_limit_key = claims
            .sub
            .as_deref()
            .map(str::trim)
            .filter(|sub| !sub.is_empty())
            .map(|sub| format!("jwt:sub:{sub}"))
            .unwrap_or_else(|| format!("jwt:token:{}", token_fingerprint(token)));
        Self {
            rate_limit_key,
            admin: claims.latlng_admin,
            permissions: claims.permissions,
        }
    }

    pub fn is_admin(&self) -> bool {
        self.admin
    }

    pub fn allows(&self, action: AuthAction, collection: &str) -> bool {
        self.admin
            || self
                .permissions
                .iter()
                .any(|rule| rule.allows(action, collection))
    }

    pub fn allows_global(&self, action: AuthAction) -> bool {
        self.admin
            || self
                .permissions
                .iter()
                .any(|rule| rule.allows_global(action))
    }

    pub fn can_view_collection(&self, collection: &str) -> bool {
        if self.admin {
            return true;
        }
        self.allows(AuthAction::CollectionsList, collection)
            && COLLECTION_VISIBILITY_ACTIONS
                .iter()
                .any(|action| self.allows(*action, collection))
    }

    pub fn any_collection_permission(&self, action: AuthAction) -> bool {
        self.admin
            || self
                .permissions
                .iter()
                .any(|rule| rule.actions.contains(&action))
    }
}

#[derive(Debug, Clone)]
pub struct Authenticator {
    config: AuthConfig,
    jwks_client: Option<Client>,
    jwks_cache: std::sync::Arc<RwLock<Option<CachedJwks>>>,
}

#[derive(Debug)]
struct CachedJwks {
    set: JwkSet,
    fetched_at: Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JwtSourceKind {
    HmacSecret,
    PublicKeyPem,
    Jwks,
}

#[derive(Debug, Error)]
pub enum AuthError {
    #[error("unauthorized")]
    Unauthorized,
    #[error("unsupported jwt algorithm: {0}")]
    UnsupportedAlgorithm(String),
    #[error("invalid auth configuration: {0}")]
    InvalidConfiguration(String),
}

#[derive(Debug, Clone, Deserialize)]
struct PermissionClaim {
    collections: Vec<String>,
    actions: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct ClaimsPayload {
    #[serde(default)]
    sub: Option<String>,
    #[serde(default)]
    latlng_permissions: Vec<PermissionClaim>,
    #[serde(default)]
    latlng_admin: bool,
}

#[derive(Debug, Clone)]
struct JwtClaims {
    sub: Option<String>,
    latlng_admin: bool,
    permissions: Vec<PermissionRule>,
}

impl AuthConfig {
    pub fn auth_enabled(&self) -> bool {
        self.bearer_enabled() || self.jwt_source_kind().is_some()
    }

    pub fn bearer_enabled(&self) -> bool {
        !self.disable_bearer_token && self.bearer_token.is_some()
    }

    pub fn validate(&self) -> Result<(), AuthError> {
        let source_count = usize::from(self.jwt_secret.is_some())
            + usize::from(self.jwt_public_key_pem.is_some())
            + usize::from(self.jwks_url.is_some());
        if source_count > 1 {
            return Err(AuthError::InvalidConfiguration(
                "exactly one JWT verification source may be configured".to_owned(),
            ));
        }

        if self.jwks_url.is_some() && self.jwks_http_timeout_ms == 0 {
            return Err(AuthError::InvalidConfiguration(
                "jwks_http_timeout_ms must be greater than zero".to_owned(),
            ));
        }

        if self.jwks_url.is_some() && self.jwks_refresh_interval_seconds == 0 {
            return Err(AuthError::InvalidConfiguration(
                "jwks_refresh_interval_seconds must be greater than zero".to_owned(),
            ));
        }

        if self.jwks_url.is_some() && self.jwks_cache_ttl_seconds == 0 {
            return Err(AuthError::InvalidConfiguration(
                "jwks_cache_ttl_seconds must be greater than zero".to_owned(),
            ));
        }

        if let Some(kind) = self.jwt_source_kind() {
            let algorithm = self.jwt_algorithm_for(kind)?;
            match (kind, algorithm) {
                (
                    JwtSourceKind::HmacSecret,
                    Algorithm::HS256 | Algorithm::HS384 | Algorithm::HS512,
                ) => {}
                (
                    JwtSourceKind::PublicKeyPem | JwtSourceKind::Jwks,
                    Algorithm::RS256 | Algorithm::ES256,
                ) => {}
                (JwtSourceKind::HmacSecret, other) => {
                    return Err(AuthError::InvalidConfiguration(format!(
                        "{} requires an HS* algorithm, got {other:?}",
                        self.jwt_source_name(kind)
                    )));
                }
                (JwtSourceKind::PublicKeyPem | JwtSourceKind::Jwks, other) => {
                    return Err(AuthError::InvalidConfiguration(format!(
                        "{} requires RS256 or ES256, got {other:?}",
                        self.jwt_source_name(kind)
                    )));
                }
            }
        }

        Ok(())
    }

    pub fn authenticator(&self) -> Result<Authenticator, AuthError> {
        self.validate()?;
        let jwks_client = if self.jwks_url.is_some() {
            Some(
                Client::builder()
                    .timeout(Duration::from_millis(self.jwks_http_timeout_ms.max(1)))
                    .build()
                    .map_err(|error| {
                        AuthError::InvalidConfiguration(format!(
                            "failed to construct JWKS client: {error}"
                        ))
                    })?,
            )
        } else {
            None
        };
        Ok(Authenticator {
            config: self.clone(),
            jwks_client,
            jwks_cache: std::sync::Arc::new(RwLock::new(None)),
        })
    }

    fn jwt_source_kind(&self) -> Option<JwtSourceKind> {
        if self.jwt_secret.is_some() {
            Some(JwtSourceKind::HmacSecret)
        } else if self.jwt_public_key_pem.is_some() {
            Some(JwtSourceKind::PublicKeyPem)
        } else if self.jwks_url.is_some() {
            Some(JwtSourceKind::Jwks)
        } else {
            None
        }
    }

    fn jwt_algorithm_for(&self, kind: JwtSourceKind) -> Result<Algorithm, AuthError> {
        let Some(value) = self.jwt_algorithm.as_deref() else {
            return Ok(match kind {
                JwtSourceKind::HmacSecret => Algorithm::HS256,
                JwtSourceKind::PublicKeyPem | JwtSourceKind::Jwks => Algorithm::RS256,
            });
        };

        match value.to_ascii_uppercase().as_str() {
            "HS256" => Ok(Algorithm::HS256),
            "HS384" => Ok(Algorithm::HS384),
            "HS512" => Ok(Algorithm::HS512),
            "RS256" => Ok(Algorithm::RS256),
            "ES256" => Ok(Algorithm::ES256),
            other => Err(AuthError::UnsupportedAlgorithm(other.to_owned())),
        }
    }

    fn jwt_source_name(&self, kind: JwtSourceKind) -> &'static str {
        match kind {
            JwtSourceKind::HmacSecret => "jwt_secret",
            JwtSourceKind::PublicKeyPem => "jwt_public_key_pem",
            JwtSourceKind::Jwks => "jwks_url",
        }
    }

    fn validation_for(&self, algorithm: Algorithm) -> Validation {
        let mut validation = Validation::new(algorithm);
        validation.leeway = self.jwt_leeway_seconds;
        if let Some(issuer) = &self.jwt_issuer {
            validation.set_issuer(&[issuer]);
        }
        if let Some(audience) = &self.jwt_audience {
            validation.set_audience(&[audience]);
        }
        validation
    }
}

impl Authenticator {
    pub fn config(&self) -> &AuthConfig {
        &self.config
    }

    pub async fn authenticate(&self, token: Option<&str>) -> Result<AuthPrincipal, AuthError> {
        if !self.config.auth_enabled() {
            return Ok(AuthPrincipal::open_access());
        }

        let token = token.ok_or(AuthError::Unauthorized)?;

        if self.config.bearer_enabled()
            && self
                .config
                .bearer_token
                .as_deref()
                .is_some_and(|expected| expected == token)
        {
            return Ok(AuthPrincipal::service_admin());
        }

        let Some(source_kind) = self.config.jwt_source_kind() else {
            return Err(AuthError::Unauthorized);
        };
        let algorithm = self.config.jwt_algorithm_for(source_kind)?;
        let claims = match source_kind {
            JwtSourceKind::HmacSecret => {
                let key = DecodingKey::from_secret(
                    self.config
                        .jwt_secret
                        .as_deref()
                        .expect("jwt_secret presence validated")
                        .as_bytes(),
                );
                self.decode_claims(token, &key, algorithm)?
            }
            JwtSourceKind::PublicKeyPem => {
                let pem = self
                    .config
                    .jwt_public_key_pem
                    .as_deref()
                    .expect("jwt_public_key_pem presence validated");
                let key = self.decoding_key_from_pem(pem.as_bytes(), algorithm)?;
                self.decode_claims(token, &key, algorithm)?
            }
            JwtSourceKind::Jwks => {
                let key = self.jwks_decoding_key(token, algorithm).await?;
                self.decode_claims(token, &key, algorithm)?
            }
        };
        Ok(AuthPrincipal::from_claims(claims, token))
    }

    fn decode_claims(
        &self,
        token: &str,
        key: &DecodingKey,
        algorithm: Algorithm,
    ) -> Result<JwtClaims, AuthError> {
        let claims = decode::<Value>(token, key, &self.config.validation_for(algorithm))
            .map_err(|_| AuthError::Unauthorized)?
            .claims;
        let payload: ClaimsPayload =
            serde_json::from_value(claims).map_err(|_| AuthError::Unauthorized)?;
        JwtClaims::from_payload(payload)
    }

    fn decoding_key_from_pem(
        &self,
        pem: &[u8],
        algorithm: Algorithm,
    ) -> Result<DecodingKey, AuthError> {
        match algorithm {
            Algorithm::RS256 => DecodingKey::from_rsa_pem(pem).map_err(|_| AuthError::Unauthorized),
            Algorithm::ES256 => DecodingKey::from_ec_pem(pem).map_err(|_| AuthError::Unauthorized),
            _ => Err(AuthError::Unauthorized),
        }
    }

    async fn jwks_decoding_key(
        &self,
        token: &str,
        algorithm: Algorithm,
    ) -> Result<DecodingKey, AuthError> {
        let header = decode_header(token).map_err(|_| AuthError::Unauthorized)?;
        let kid = header.kid.as_deref().ok_or(AuthError::Unauthorized)?;
        let set = self.current_jwks().await?;
        let jwk = set.find(kid).ok_or(AuthError::Unauthorized)?;
        match algorithm {
            Algorithm::RS256 | Algorithm::ES256 => {
                DecodingKey::from_jwk(jwk).map_err(|_| AuthError::Unauthorized)
            }
            _ => Err(AuthError::Unauthorized),
        }
    }

    async fn current_jwks(&self) -> Result<JwkSet, AuthError> {
        let url = self.config.jwks_url.as_deref().ok_or_else(|| {
            AuthError::InvalidConfiguration("jwks_url is not configured".to_owned())
        })?;
        let client = self.jwks_client.as_ref().ok_or_else(|| {
            AuthError::InvalidConfiguration("JWKS client is not configured".to_owned())
        })?;
        let refresh_after = Duration::from_secs(self.config.jwks_refresh_interval_seconds.max(1));
        let cache_ttl = Duration::from_secs(self.config.jwks_cache_ttl_seconds.max(1));

        {
            let cache = self.jwks_cache.read().await;
            if let Some(cache) = cache.as_ref()
                && cache.fetched_at.elapsed() < refresh_after
            {
                return Ok(cache.set.clone());
            }
        }

        let response = client
            .get(url)
            .send()
            .await
            .map_err(|_| AuthError::Unauthorized)?;
        let response = response
            .error_for_status()
            .map_err(|_| AuthError::Unauthorized)?;
        let set = response
            .json::<JwkSet>()
            .await
            .map_err(|_| AuthError::Unauthorized)?;

        {
            let mut cache = self.jwks_cache.write().await;
            *cache = Some(CachedJwks {
                set: set.clone(),
                fetched_at: Instant::now(),
            });
        }

        let cache = self.jwks_cache.read().await;
        let Some(cache) = cache.as_ref() else {
            return Err(AuthError::Unauthorized);
        };
        if cache.fetched_at.elapsed() > cache_ttl {
            return Err(AuthError::Unauthorized);
        }
        Ok(cache.set.clone())
    }
}

impl JwtClaims {
    fn from_payload(payload: ClaimsPayload) -> Result<Self, AuthError> {
        let mut permissions = Vec::with_capacity(payload.latlng_permissions.len());
        for claim in payload.latlng_permissions {
            if claim.collections.is_empty() || claim.actions.is_empty() {
                return Err(AuthError::Unauthorized);
            }
            let mut actions = HashSet::new();
            for action in claim.actions {
                let Some(parsed) = AuthAction::parse(action.as_str()) else {
                    return Err(AuthError::Unauthorized);
                };
                actions.insert(parsed);
            }
            permissions.push(PermissionRule {
                patterns: claim.collections,
                actions,
            });
        }
        Ok(Self {
            sub: payload.sub,
            latlng_admin: payload.latlng_admin,
            permissions,
        })
    }
}

fn token_fingerprint(token: &str) -> String {
    let digest = Sha256::digest(token.as_bytes());
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(&mut output, "{byte:02x}");
    }
    output
}

pub fn extract_bearer_token(header_value: &str) -> Option<&str> {
    header_value.strip_prefix("Bearer ")
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use axum::{Json, Router, routing::get};
    use jsonwebtoken::{EncodingKey, Header, encode};
    use serde_json::json;
    use tokio::net::TcpListener;

    use super::{AuthAction, AuthConfig, AuthError, extract_bearer_token};

    fn exp_offset(seconds: i64) -> usize {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        (now + seconds) as usize
    }

    async fn jwks_server(body: serde_json::Value) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let app = Router::new().route("/jwks", get(|| async move { Json(body) }));
            axum::serve(listener, app).await.unwrap();
        });
        format!("http://{addr}/jwks")
    }

    fn rsa_private_key() -> &'static [u8] {
        br#"-----BEGIN RSA PRIVATE KEY-----
MIIEpAIBAAKCAQEAyRE6rHuNR0QbHO3H3Kt2pOKGVhQqGZXInOduQNxXzuKlvQTL
UTv4l4sggh5/CYYi/cvI+SXVT9kPWSKXxJXBXd/4LkvcPuUakBoAkfh+eiFVMh2V
rUyWyj3MFl0HTVF9KwRXLAcwkREiS3npThHRyIxuy0ZMeZfxVL5arMhw1SRELB8H
oGfG/AtH89BIE9jDBHZ9dLelK9a184zAf8LwoPLxvJb3Il5nncqPcSfKDDodMFBI
Mc4lQzDKL5gvmiXLXB1AGLm8KBjfE8s3L5xqi+yUod+j8MtvIj812dkS4QMiRVN/
by2h3ZY8LYVGrqZXZTcgn2ujn8uKjXLZVD5TdQIDAQABAoIBAHREk0I0O9DvECKd
WUpAmF3mY7oY9PNQiu44Yaf+AoSuyRpRUGTMIgc3u3eivOE8ALX0BmYUO5JtuRNZ
Dpvt4SAwqCnVUinIf6C+eH/wSurCpapSM0BAHp4aOA7igptyOMgMPYBHNA1e9A7j
E0dCxKWMl3DSWNyjQTk4zeRGEAEfbNjHrq6YCtjHSZSLmWiG80hnfnYos9hOr5Jn
LnyS7ZmFE/5P3XVrxLc/tQ5zum0R4cbrgzHiQP5RgfxGJaEi7XcgherCCOgurJSS
bYH29Gz8u5fFbS+Yg8s+OiCss3cs1rSgJ9/eHZuzGEdUZVARH6hVMjSuwvqVTFaE
8AgtleECgYEA+uLMn4kNqHlJS2A5uAnCkj90ZxEtNm3E8hAxUrhssktY5XSOAPBl
xyf5RuRGIImGtUVIr4HuJSa5TX48n3Vdt9MYCprO/iYl6moNRSPt5qowIIOJmIjY
2mqPDfDt/zw+fcDD3lmCJrFlzcnh0uea1CohxEbQnL3cypeLt+WbU6kCgYEAzSp1
9m1ajieFkqgoB0YTpt/OroDx38vvI5unInJlEeOjQ+oIAQdN2wpxBvTrRorMU6P0
7mFUbt1j+Co6CbNiw+X8HcCaqYLR5clbJOOWNR36PuzOpQLkfK8woupBxzW9B8gZ
mY8rB1mbJ+/WTPrEJy6YGmIEBkWylQ2VpW8O4O0CgYEApdbvvfFBlwD9YxbrcGz7
MeNCFbMz+MucqQntIKoKJ91ImPxvtc0y6e/Rhnv0oyNlaUOwJVu0yNgNG117w0g4
t/+Q38mvVC5xV7/cn7x9UMFk6MkqVir3dYGEqIl/OP1grY2Tq9HtB5iyG9L8NIam
QOLMyUqqMUILxdthHyFmiGkCgYEAn9+PjpjGMPHxL0gj8Q8VbzsFtou6b1deIRRA
2CHmSltltR1gYVTMwXxQeUhPMmgkMqUXzs4/WijgpthY44hK1TaZEKIuoxrS70nJ
4WQLf5a9k1065fDsFZD6yGjdGxvwEmlGMZgTwqV7t1I4X0Ilqhav5hcs5apYL7gn
PYPeRz0CgYALHCj/Ji8XSsDoF/MhVhnGdIs2P99NNdmo3R2Pv0CuZbDKMU559LJH
UvrKS8WkuWRDuKrz1W/EQKApFjDGpdqToZqriUFQzwy7mR3ayIiogzNtHcvbDHx8
oFnGY0OFksX/ye0/XGpy2SFxYRwGU98HPYeBvAQQrVjdkzfy7BmXQQ==
-----END RSA PRIVATE KEY-----"#
    }

    fn rsa_public_key() -> &'static [u8] {
        br#"-----BEGIN RSA PUBLIC KEY-----
MIIBCgKCAQEAyRE6rHuNR0QbHO3H3Kt2pOKGVhQqGZXInOduQNxXzuKlvQTLUTv4
l4sggh5/CYYi/cvI+SXVT9kPWSKXxJXBXd/4LkvcPuUakBoAkfh+eiFVMh2VrUyW
yj3MFl0HTVF9KwRXLAcwkREiS3npThHRyIxuy0ZMeZfxVL5arMhw1SRELB8HoGfG
/AtH89BIE9jDBHZ9dLelK9a184zAf8LwoPLxvJb3Il5nncqPcSfKDDodMFBIMc4l
QzDKL5gvmiXLXB1AGLm8KBjfE8s3L5xqi+yUod+j8MtvIj812dkS4QMiRVN/by2h
3ZY8LYVGrqZXZTcgn2ujn8uKjXLZVD5TdQIDAQAB
-----END RSA PUBLIC KEY-----"#
    }

    fn ec_private_key() -> &'static [u8] {
        br#"-----BEGIN PRIVATE KEY-----
MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgWTFfCGljY6aw3Hrt
kHmPRiazukxPLb6ilpRAewjW8nihRANCAATDskChT+Altkm9X7MI69T3IUmrQU0L
950IxEzvw/x5BMEINRMrXLBJhqzO9Bm+d6JbqA21YQmd1Kt4RzLJR1W+
-----END PRIVATE KEY-----"#
    }

    fn ec_public_key() -> &'static [u8] {
        br#"-----BEGIN PUBLIC KEY-----
MFkwEwYHKoZIzj0CAQYIKoZIzj0DAQcDQgAEw7JAoU/gJbZJvV+zCOvU9yFJq0FN
C/edCMRM78P8eQTBCDUTK1ywSYaszvQZvneiW6gNtWEJndSreEcyyUdVvg==
-----END PUBLIC KEY-----"#
    }

    #[tokio::test]
    async fn shared_secret_jwt_respects_configured_algorithm() {
        let token = encode(
            &Header::new(jsonwebtoken::Algorithm::HS512),
            &json!({ "sub": "demo", "exp": exp_offset(60) }),
            &EncodingKey::from_secret(b"secret"),
        )
        .unwrap();

        let auth = AuthConfig {
            jwt_secret: Some("secret".to_owned()),
            jwt_algorithm: Some("HS512".to_owned()),
            ..AuthConfig::default()
        }
        .authenticator()
        .unwrap();

        auth.authenticate(Some(&token)).await.unwrap();
    }

    #[tokio::test]
    async fn jwt_leeway_allows_recently_expired_tokens() {
        let token = encode(
            &Header::new(jsonwebtoken::Algorithm::HS256),
            &json!({ "sub": "demo", "exp": exp_offset(-2) }),
            &EncodingKey::from_secret(b"secret"),
        )
        .unwrap();

        let auth = AuthConfig {
            jwt_secret: Some("secret".to_owned()),
            jwt_leeway_seconds: 5,
            ..AuthConfig::default()
        }
        .authenticator()
        .unwrap();

        auth.authenticate(Some(&token)).await.unwrap();
    }

    #[tokio::test]
    async fn bearer_token_can_be_disabled() {
        let token = encode(
            &Header::new(jsonwebtoken::Algorithm::HS256),
            &json!({ "sub": "demo", "exp": exp_offset(60) }),
            &EncodingKey::from_secret(b"secret"),
        )
        .unwrap();

        let auth = AuthConfig {
            bearer_token: Some("secret-bearer".to_owned()),
            disable_bearer_token: true,
            jwt_secret: Some("secret".to_owned()),
            ..AuthConfig::default()
        }
        .authenticator()
        .unwrap();

        match auth.authenticate(Some("secret-bearer")).await {
            Err(AuthError::Unauthorized) => {}
            other => panic!("unexpected result: {other:?}"),
        }
        auth.authenticate(Some(&token)).await.unwrap();
    }

    #[test]
    fn rejects_multiple_jwt_sources() {
        let config = AuthConfig {
            jwt_secret: Some("secret".to_owned()),
            jwt_public_key_pem: Some("pem".to_owned()),
            ..AuthConfig::default()
        };
        match config.validate() {
            Err(AuthError::InvalidConfiguration(message)) => {
                assert!(message.contains("exactly one JWT verification source"));
            }
            other => panic!("unexpected result: {other:?}"),
        }
    }

    #[tokio::test]
    async fn pem_based_rsa_validation_works() {
        let token = encode(
            &Header::new(jsonwebtoken::Algorithm::RS256),
            &json!({ "sub": "demo", "exp": exp_offset(60) }),
            &EncodingKey::from_rsa_pem(rsa_private_key()).unwrap(),
        )
        .unwrap();
        let auth = AuthConfig {
            jwt_public_key_pem: Some(String::from_utf8(rsa_public_key().to_vec()).unwrap()),
            jwt_algorithm: Some("RS256".to_owned()),
            ..AuthConfig::default()
        }
        .authenticator()
        .unwrap();
        auth.authenticate(Some(&token)).await.unwrap();
    }

    #[tokio::test]
    async fn pem_based_ec_validation_works() {
        let token = encode(
            &Header::new(jsonwebtoken::Algorithm::ES256),
            &json!({ "sub": "demo", "exp": exp_offset(60) }),
            &EncodingKey::from_ec_pem(ec_private_key()).unwrap(),
        )
        .unwrap();
        let auth = AuthConfig {
            jwt_public_key_pem: Some(String::from_utf8(ec_public_key().to_vec()).unwrap()),
            jwt_algorithm: Some("ES256".to_owned()),
            ..AuthConfig::default()
        }
        .authenticator()
        .unwrap();
        auth.authenticate(Some(&token)).await.unwrap();
    }

    #[tokio::test]
    async fn jwks_validation_works() {
        let token = encode(
            &Header {
                kid: Some("rsa01".to_owned()),
                ..Header::new(jsonwebtoken::Algorithm::RS256)
            },
            &json!({ "sub": "demo", "exp": exp_offset(60) }),
            &EncodingKey::from_rsa_pem(rsa_private_key()).unwrap(),
        )
        .unwrap();
        let url = jwks_server(json!({
            "keys": [{
                "kty": "RSA",
                "n": "yRE6rHuNR0QbHO3H3Kt2pOKGVhQqGZXInOduQNxXzuKlvQTLUTv4l4sggh5_CYYi_cvI-SXVT9kPWSKXxJXBXd_4LkvcPuUakBoAkfh-eiFVMh2VrUyWyj3MFl0HTVF9KwRXLAcwkREiS3npThHRyIxuy0ZMeZfxVL5arMhw1SRELB8HoGfG_AtH89BIE9jDBHZ9dLelK9a184zAf8LwoPLxvJb3Il5nncqPcSfKDDodMFBIMc4lQzDKL5gvmiXLXB1AGLm8KBjfE8s3L5xqi-yUod-j8MtvIj812dkS4QMiRVN_by2h3ZY8LYVGrqZXZTcgn2ujn8uKjXLZVD5TdQ",
                "e": "AQAB",
                "kid": "rsa01",
                "alg": "RS256",
                "use": "sig"
            }]
        }))
        .await;

        let auth = AuthConfig {
            jwt_algorithm: Some("RS256".to_owned()),
            jwks_url: Some(url),
            ..AuthConfig::default()
        }
        .authenticator()
        .unwrap();
        auth.authenticate(Some(&token)).await.unwrap();
    }

    #[tokio::test]
    async fn parses_collection_permissions() {
        let token = encode(
            &Header::new(jsonwebtoken::Algorithm::HS256),
            &json!({
                "sub": "demo",
                "exp": exp_offset(60),
                "latlng_permissions": [
                    {
                        "collections": ["fleet-*"],
                        "actions": ["collections:list", "objects:read", "queries:read"]
                    },
                    {
                        "collections": ["*"],
                        "actions": ["metrics:read"]
                    }
                ]
            }),
            &EncodingKey::from_secret(b"secret"),
        )
        .unwrap();

        let auth = AuthConfig {
            jwt_secret: Some("secret".to_owned()),
            ..AuthConfig::default()
        }
        .authenticator()
        .unwrap();
        let principal = auth.authenticate(Some(&token)).await.unwrap();
        assert_eq!(principal.rate_limit_key, "jwt:sub:demo");
        assert!(principal.allows(AuthAction::ObjectsRead, "fleet-eu"));
        assert!(principal.can_view_collection("fleet-eu"));
        assert!(!principal.allows(AuthAction::ObjectsRead, "other"));
        assert!(principal.allows_global(AuthAction::MetricsRead));
    }

    #[tokio::test]
    async fn principal_rate_limit_keys_are_stable_and_non_secret() {
        let auth = AuthConfig {
            bearer_token: Some("secret-bearer".to_owned()),
            jwt_secret: Some("secret".to_owned()),
            ..AuthConfig::default()
        }
        .authenticator()
        .unwrap();

        let with_sub = encode(
            &Header::new(jsonwebtoken::Algorithm::HS256),
            &json!({ "sub": "demo", "exp": exp_offset(60) }),
            &EncodingKey::from_secret(b"secret"),
        )
        .unwrap();
        let without_sub = encode(
            &Header::new(jsonwebtoken::Algorithm::HS256),
            &json!({ "exp": exp_offset(60) }),
            &EncodingKey::from_secret(b"secret"),
        )
        .unwrap();

        assert_eq!(
            auth.authenticate(Some(&with_sub))
                .await
                .unwrap()
                .rate_limit_key,
            "jwt:sub:demo"
        );
        let first = auth
            .authenticate(Some(&without_sub))
            .await
            .unwrap()
            .rate_limit_key;
        let second = auth
            .authenticate(Some(&without_sub))
            .await
            .unwrap()
            .rate_limit_key;
        assert_eq!(first, second);
        assert!(first.starts_with("jwt:token:"));
        assert!(!first.contains(&without_sub));
        assert_eq!(
            auth.authenticate(Some("secret-bearer"))
                .await
                .unwrap()
                .rate_limit_key,
            "bearer:service"
        );

        let open = AuthConfig::default().authenticator().unwrap();
        assert_eq!(
            open.authenticate(None).await.unwrap().rate_limit_key,
            "open:access"
        );
    }

    #[test]
    fn extracts_bearer_tokens() {
        assert_eq!(extract_bearer_token("Bearer secret"), Some("secret"));
        assert_eq!(extract_bearer_token("Token secret"), None);
    }
}
