use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{bail, Context};
use axum::{
    body::Body,
    extract::{Query, State},
    http::{
        header::{COOKIE, SET_COOKIE},
        HeaderMap, HeaderValue, Request, StatusCode,
    },
    middleware::Next,
    response::{IntoResponse, Redirect, Response},
    routing::get,
    Extension, Json, Router,
};
use chrono::Utc;
use openidconnect::{
    core::{CoreAuthenticationFlow, CoreClient, CoreProviderMetadata},
    reqwest,
    AccessTokenHash, AuthorizationCode, ClientId, ClientSecret, CsrfToken, IssuerUrl,
    Nonce, OAuth2TokenResponse, PkceCodeChallenge, PkceCodeVerifier, RedirectUrl, Scope,
    TokenResponse,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::{Row, SqlitePool};
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::app_state::AppState;

const SESSION_COOKIE_NAME: &str = "mdev_session";
const LOGIN_TTL: Duration = Duration::from_secs(10 * 60);
const SESSION_TTL: Duration = Duration::from_secs(8 * 60 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AuthMode {
    Disabled,
    Entra,
}

#[derive(Clone)]
pub struct AuthState {
    db: SqlitePool,
    mode: AuthMode,
    provider_metadata: Option<Arc<CoreProviderMetadata>>,
    http_client: reqwest::Client,
    client_id: String,
    client_secret: String,
    redirect_uri: String,
    post_login_url: String,
    cookie_secure: bool,
    pending_logins: Arc<RwLock<HashMap<String, PendingLogin>>>,
    sessions: Arc<RwLock<HashMap<String, SessionRecord>>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AuthenticatedUser {
    pub id: String,
    pub email: String,
    pub display_name: String,
}

#[derive(Debug, Clone)]
struct RequestAuth {
    auth_enabled: bool,
    user: Option<AuthenticatedUser>,
}

#[derive(Debug)]
struct PendingLogin {
    nonce: String,
    pkce_verifier: String,
    created_at: Instant,
}

#[derive(Debug, Clone)]
struct SessionRecord {
    user: AuthenticatedUser,
    expires_at: Instant,
}

#[derive(Debug, Deserialize)]
struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

pub async fn initialize(db: &SqlitePool) -> anyhow::Result<AuthState> {
    let mode = match std::env::var("MDEV_AUTH_MODE")
        .unwrap_or_else(|_| "disabled".to_string())
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "" | "disabled" | "off" => AuthMode::Disabled,
        "entra" | "entra_oidc" | "entra-oidc" => AuthMode::Entra,
        value => bail!("unsupported MDEV_AUTH_MODE '{value}'"),
    };

    seed_bootstrap_users(db).await?;

    let http_client = reqwest::ClientBuilder::new()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .context("failed to build Entra OIDC HTTP client")?;

    let mut client_id = String::new();
    let mut client_secret = String::new();
    let mut redirect_uri = String::new();
    let mut post_login_url = "/".to_string();
    let mut provider_metadata = None;
    let mut cookie_secure = false;

    if mode == AuthMode::Entra {
        let tenant_id = required_env("MDEV_ENTRA_TENANT_ID")?;
        client_id = required_env("MDEV_ENTRA_CLIENT_ID")?;
        client_secret = required_env("MDEV_ENTRA_CLIENT_SECRET")?;
        redirect_uri = required_env("MDEV_ENTRA_REDIRECT_URI")?;
        post_login_url = std::env::var("MDEV_ENTRA_POST_LOGIN_URL")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "/".to_string());

        RedirectUrl::new(redirect_uri.clone())
            .context("invalid MDEV_ENTRA_REDIRECT_URI")?;

        cookie_secure = redirect_uri
            .trim_start()
            .to_ascii_lowercase()
            .starts_with("https://");

        let enabled_users: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM app_users WHERE enabled = 1")
                .fetch_one(db)
                .await?;

        if enabled_users == 0 {
            bail!("MDEV_AUTH_MODE=entra requires at least one enabled app_users row or MDEV_AUTH_BOOTSTRAP_USERS");
        }

        let issuer = IssuerUrl::new(format!(
            "https://login.microsoftonline.com/{}/v2.0",
            tenant_id.trim()
        ))
        .context("invalid Microsoft Entra issuer URL")?;

        let discovered = CoreProviderMetadata::discover_async(issuer, &http_client)
            .await
            .context("failed to discover Microsoft Entra OpenID Connect metadata")?;

        provider_metadata = Some(Arc::new(discovered));
    }

    Ok(AuthState {
        db: db.clone(),
        mode,
        provider_metadata,
        http_client,
        client_id,
        client_secret,
        redirect_uri,
        post_login_url,
        cookie_secure,
        pending_logins: Arc::new(RwLock::new(HashMap::new())),
        sessions: Arc::new(RwLock::new(HashMap::new())),
    })
}

impl AuthState {
    pub fn enabled(&self) -> bool {
        self.mode != AuthMode::Disabled
    }
}

fn required_env(name: &str) -> anyhow::Result<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .with_context(|| format!("{name} is required when MDEV_AUTH_MODE=entra"))
}

fn normalize_email(value: &str) -> Option<String> {
    let email = value.trim().to_ascii_lowercase();
    let (local, domain) = email.split_once('@')?;
    if local.is_empty() || domain.is_empty() || domain.contains('@') {
        return None;
    }
    Some(email)
}

async fn seed_bootstrap_users(db: &SqlitePool) -> anyhow::Result<()> {
    let raw = std::env::var("MDEV_AUTH_BOOTSTRAP_USERS").unwrap_or_default();

    for value in raw
        .split(|ch| matches!(ch, ',' | ';' | '\n' | '\r'))
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let email = normalize_email(value)
            .with_context(|| format!("invalid email/UPN in MDEV_AUTH_BOOTSTRAP_USERS: {value}"))?;
        let now = Utc::now().to_rfc3339();

        sqlx::query(
            r#"
            INSERT INTO app_users (id, email, display_name, enabled, created_at, updated_at)
            VALUES (?, ?, '', 1, ?, ?)
            ON CONFLICT(email) DO NOTHING
            "#,
        )
        .bind(Uuid::new_v4().to_string())
        .bind(email)
        .bind(&now)
        .bind(&now)
        .execute(db)
        .await?;
    }

    Ok(())
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/auth/me", get(me))
        .route("/api/auth/login", get(login))
        .route("/api/auth/callback", get(callback))
        .route("/api/auth/logout", get(logout))
}

pub async fn authorize(
    State(state): State<AuthState>,
    mut request: Request<Body>,
    next: Next,
) -> Response {
    let path = request.uri().path();

    if matches!(
        path,
        "/api/health" | "/api/auth/login" | "/api/auth/callback"
    ) {
        return next.run(request).await;
    }

    if state.mode == AuthMode::Disabled {
        request.extensions_mut().insert(RequestAuth {
            auth_enabled: false,
            user: None,
        });
        return next.run(request).await;
    }

    let Some(session_id) = cookie_value(request.headers(), SESSION_COOKIE_NAME) else {
        return auth_error(StatusCode::UNAUTHORIZED, "authentication required");
    };

    let session = {
        let sessions = state.sessions.read().await;
        sessions.get(&session_id).cloned()
    };

    let Some(session) = session else {
        return auth_error(StatusCode::UNAUTHORIZED, "authentication required");
    };

    if Instant::now() >= session.expires_at {
        state.sessions.write().await.remove(&session_id);
        return auth_error(StatusCode::UNAUTHORIZED, "authentication session expired");
    }

    request.extensions_mut().insert(RequestAuth {
        auth_enabled: true,
        user: Some(session.user),
    });

    next.run(request).await
}

async fn login(Extension(state): Extension<AuthState>) -> Response {
    if state.mode == AuthMode::Disabled {
        return Redirect::to("/").into_response();
    }

    let Some(provider_metadata) = state.provider_metadata.as_ref() else {
        return auth_error(StatusCode::INTERNAL_SERVER_ERROR, "Entra authentication is not initialized");
    };

    let redirect_uri = match RedirectUrl::new(state.redirect_uri.clone()) {
        Ok(value) => value,
        Err(error) => {
            tracing::error!(error = %error, "invalid Entra redirect URI");
            return auth_error(StatusCode::INTERNAL_SERVER_ERROR, "invalid authentication configuration");
        }
    };

    let client = CoreClient::from_provider_metadata(
        provider_metadata.as_ref().clone(),
        ClientId::new(state.client_id.clone()),
        Some(ClientSecret::new(state.client_secret.clone())),
    )
    .set_redirect_uri(redirect_uri);

    let (pkce_challenge, pkce_verifier) = PkceCodeChallenge::new_random_sha256();

    let (authorization_url, csrf_token, nonce) = client
        .authorize_url(
            CoreAuthenticationFlow::AuthorizationCode,
            CsrfToken::new_random,
            Nonce::new_random,
        )
        .add_scope(Scope::new("profile".to_string()))
        .add_scope(Scope::new("email".to_string()))
        .set_pkce_challenge(pkce_challenge)
        .url();

    state.pending_logins.write().await.insert(
        csrf_token.secret().to_string(),
        PendingLogin {
            nonce: nonce.secret().to_string(),
            pkce_verifier: pkce_verifier.secret().to_string(),
            created_at: Instant::now(),
        },
    );

    Redirect::temporary(authorization_url.as_str()).into_response()
}

async fn callback(
    Extension(state): Extension<AuthState>,
    Query(query): Query<CallbackQuery>,
) -> Response {
    if state.mode != AuthMode::Entra {
        return Redirect::to("/").into_response();
    }

    if let Some(error) = query.error.as_deref() {
        tracing::warn!(
            error,
            description = query.error_description.as_deref().unwrap_or(""),
            "Microsoft Entra authentication returned an error"
        );
        return auth_error(StatusCode::UNAUTHORIZED, "Microsoft Entra authentication was not completed");
    }

    let Some(state_value) = query.state.as_deref() else {
        return auth_error(StatusCode::BAD_REQUEST, "missing authentication state");
    };

    let Some(code) = query.code.as_deref() else {
        return auth_error(StatusCode::BAD_REQUEST, "missing authorization code");
    };

    let pending = state.pending_logins.write().await.remove(state_value);
    let Some(pending) = pending else {
        return auth_error(StatusCode::UNAUTHORIZED, "invalid or already-used authentication state");
    };

    if pending.created_at.elapsed() > LOGIN_TTL {
        return auth_error(StatusCode::UNAUTHORIZED, "authentication attempt expired");
    }

    let user = match complete_entra_login(&state, code, pending).await {
        Ok(Some(user)) => user,
        Ok(None) => {
            return auth_error(StatusCode::FORBIDDEN, "user is not allowed to access MDEV");
        }
        Err(error) => {
            tracing::warn!(error = %format!("{:#}", error), "Microsoft Entra authentication failed");
            return auth_error(StatusCode::UNAUTHORIZED, "Microsoft Entra authentication failed");
        }
    };

    let session_id = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());

    state.sessions.write().await.insert(
        session_id.clone(),
        SessionRecord {
            user: user.clone(),
            expires_at: Instant::now() + SESSION_TTL,
        },
    );

    tracing::info!(
        user_id = %user.id,
        email = %user.email,
        "Microsoft Entra authentication succeeded"
    );

    let mut response = Redirect::to(&state.post_login_url).into_response();
    let cookie = session_cookie(&session_id, state.cookie_secure);

    match HeaderValue::from_str(&cookie) {
        Ok(value) => {
            response.headers_mut().append(SET_COOKIE, value);
            response
        }
        Err(error) => {
            tracing::error!(error = %error, "failed to construct authentication session cookie");
            auth_error(StatusCode::INTERNAL_SERVER_ERROR, "failed to create authentication session")
        }
    }
}

async fn complete_entra_login(
    state: &AuthState,
    code: &str,
    pending: PendingLogin,
) -> anyhow::Result<Option<AuthenticatedUser>> {
    let provider_metadata = state
        .provider_metadata
        .as_ref()
        .context("Entra provider metadata is unavailable")?;

    let client = CoreClient::from_provider_metadata(
        provider_metadata.as_ref().clone(),
        ClientId::new(state.client_id.clone()),
        Some(ClientSecret::new(state.client_secret.clone())),
    )
    .set_redirect_uri(
        RedirectUrl::new(state.redirect_uri.clone())
            .context("invalid MDEV_ENTRA_REDIRECT_URI")?,
    );

    let token_response = client
        .exchange_code(AuthorizationCode::new(code.to_string()))?
        .set_pkce_verifier(PkceCodeVerifier::new(pending.pkce_verifier))
        .request_async(&state.http_client)
        .await
        .context("failed to exchange Microsoft Entra authorization code")?;

    let id_token = token_response
        .id_token()
        .context("Microsoft Entra token response did not contain an ID token")?;

    let verifier = client.id_token_verifier();
    let nonce = Nonce::new(pending.nonce);
    let claims = id_token
        .claims(&verifier, &nonce)
        .context("Microsoft Entra ID token validation failed")?;

    if let Some(expected_access_token_hash) = claims.access_token_hash() {
        let actual_access_token_hash = AccessTokenHash::from_token(
            token_response.access_token(),
            id_token.signing_alg()?,
            id_token.signing_key(&verifier)?,
        )?;

        if actual_access_token_hash != *expected_access_token_hash {
            bail!("Microsoft Entra access token hash validation failed");
        }
    }

    let raw_identity = claims
        .email()
        .map(|value| value.as_str())
        .or_else(|| claims.preferred_username().map(|value| value.as_str()))
        .context("Microsoft Entra ID token did not contain an email or preferred_username claim")?;

    let email = normalize_email(raw_identity)
        .context("Microsoft Entra identity is not a valid email/UPN")?;

    let row = sqlx::query(
        "SELECT id, email, display_name FROM app_users WHERE email = ? COLLATE NOCASE AND enabled = 1 LIMIT 1",
    )
    .bind(&email)
    .fetch_optional(&state.db)
    .await
    .context("failed to resolve authenticated app user")?;

    Ok(row.map(|row| AuthenticatedUser {
        id: row.get("id"),
        email: row.get("email"),
        display_name: row.get("display_name"),
    }))
}

async fn logout(
    Extension(state): Extension<AuthState>,
    headers: HeaderMap,
) -> Response {
    if let Some(session_id) = cookie_value(&headers, SESSION_COOKIE_NAME) {
        state.sessions.write().await.remove(&session_id);
    }

    let mut response = Redirect::to(&state.post_login_url).into_response();
    let cookie = expired_session_cookie(state.cookie_secure);

    if let Ok(value) = HeaderValue::from_str(&cookie) {
        response.headers_mut().append(SET_COOKIE, value);
    }

    response
}

fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get_all(COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .filter_map(|part| part.trim().split_once('='))
        .find_map(|(key, value)| {
            if key.trim() == name {
                Some(value.trim().to_string())
            } else {
                None
            }
        })
}

fn session_cookie(session_id: &str, secure: bool) -> String {
    let secure_attribute = if secure { "; Secure" } else { "" };
    format!(
        "{}={}; Path=/; HttpOnly; SameSite=Lax; Max-Age={}{}",
        SESSION_COOKIE_NAME,
        session_id,
        SESSION_TTL.as_secs(),
        secure_attribute
    )
}

fn expired_session_cookie(secure: bool) -> String {
    let secure_attribute = if secure { "; Secure" } else { "" };
    format!(
        "{}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0{}",
        SESSION_COOKIE_NAME,
        secure_attribute
    )
}

fn auth_error(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({ "ok": false, "error": message }))).into_response()
}

async fn me(Extension(auth): Extension<RequestAuth>) -> Json<serde_json::Value> {
    match auth.user {
        Some(user) => Json(json!({
            "ok": true,
            "authenticated": true,
            "user": user
        })),
        None => Json(json!({
            "ok": true,
            "authenticated": false,
            "auth_mode": if auth.auth_enabled { "entra" } else { "disabled" }
        })),
    }
}
