use axum::{
    extract::Request,
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Redirect, Response},
};
use sha2::{Sha256, Digest};

use crate::db::DbPool;

/// Hash an API key using SHA-256.
pub fn hash_key(raw_key: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(raw_key.as_bytes());
    hex::encode(hasher.finalize())
}

/// Generate a new random API key (base64-encoded 32 bytes).
pub fn generate_raw_key() -> String {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    let bytes: Vec<u8> = (0..32).map(|_| rng.gen()).collect();
    format!("mcp_{}", base64::Engine::encode(
        &base64::engine::general_purpose::URL_SAFE_NO_PAD,
        &bytes,
    ))
}

/// Validate an API key against the database. Returns true if valid.
pub fn validate_key(db: &DbPool, raw_key: &str) -> bool {
    let key_hash = hash_key(raw_key);
    let conn = db.get().unwrap();
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM api_keys WHERE key_hash = ?1 AND revoked = 0",
            [&key_hash],
            |row| row.get(0),
        )
        .unwrap_or(0);

    if count > 0 {
        // Update last_used_at
        let _ = conn.execute(
            "UPDATE api_keys SET last_used_at = datetime('now') WHERE key_hash = ?1",
            [&key_hash],
        );
    }

    count > 0
}

/// Extract the API key from the Authorization header or X-API-Key header.
fn extract_api_key(req: &Request) -> Option<String> {
    // Try Authorization: Bearer <key>
    if let Some(auth) = req.headers().get("authorization") {
        if let Ok(auth_str) = auth.to_str() {
            if let Some(key) = auth_str.strip_prefix("Bearer ") {
                return Some(key.trim().to_string());
            }
        }
    }

    // Try X-API-Key header
    if let Some(key) = req.headers().get("x-api-key") {
        if let Ok(key_str) = key.to_str() {
            return Some(key_str.trim().to_string());
        }
    }

    // Try cookie-based session (api_key cookie)
    if let Some(cookie) = req.headers().get("cookie") {
        if let Ok(cookie_str) = cookie.to_str() {
            for part in cookie_str.split(';') {
                let part = part.trim();
                if let Some(val) = part.strip_prefix("mcpolly_key=") {
                    return Some(val.trim().to_string());
                }
            }
        }
    }

    None
}

/// Axum middleware that requires a valid API key (returns 401 for API routes).
pub async fn require_api_key(
    req: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let db = req.extensions().get::<DbPool>().cloned();

    let db = match db {
        Some(db) => db,
        None => {
            tracing::error!("DbPool not found in request extensions");
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    };

    match extract_api_key(&req) {
        Some(key) if validate_key(&db, &key) => Ok(next.run(req).await),
        Some(_) => {
            tracing::warn!("Invalid API key provided");
            Err(StatusCode::UNAUTHORIZED)
        }
        None => {
            tracing::warn!("No API key provided");
            Err(StatusCode::UNAUTHORIZED)
        }
    }
}

/// Axum middleware for UI routes — redirects to /login if not authenticated.
pub async fn require_auth_or_redirect(
    req: Request,
    next: Next,
) -> Result<Response, Response> {
    let db = req.extensions().get::<DbPool>().cloned();

    let db = match db {
        Some(db) => db,
        None => {
            tracing::error!("DbPool not found in request extensions");
            return Err(Redirect::to("/login").into_response());
        }
    };

    match extract_api_key(&req) {
        Some(key) if validate_key(&db, &key) => Ok(next.run(req).await),
        _ => Err(Redirect::to("/login").into_response()),
    }
}
