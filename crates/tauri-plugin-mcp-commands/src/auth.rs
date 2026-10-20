//! Bearer-token auth and browser rejection, in front of the MCP service.

use std::{
    fs, io,
    path::Path,
    sync::{Arc, RwLock},
};

use axum::{
    extract::{Request, State},
    http::{HeaderValue, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};

use crate::secure_fs;

const TOKEN_FILE: &str = "token";
/// 32 random bytes, base64url without padding.
const TOKEN_LEN: usize = 43;

pub(crate) fn generate_token() -> io::Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|e| io::Error::other(format!("no system randomness: {e}")))?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

fn is_well_formed(token: &str) -> bool {
    token.len() == TOKEN_LEN && token.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Reads the persisted token, creating one on first start. The token outlives
/// restarts so client configurations keep working.
pub(crate) fn load_or_create(dir: &Path) -> io::Result<String> {
    let path = dir.join(TOKEN_FILE);
    match fs::read_to_string(&path) {
        Ok(t) if is_well_formed(t.trim()) && secure_fs::is_owner_only(&path).unwrap_or(false) => {
            Ok(t.trim().to_owned())
        }
        // Missing, malformed or readable by others: replace it.
        Ok(_) => rotate(dir),
        Err(e) if e.kind() == io::ErrorKind::NotFound => rotate(dir),
        Err(e) => Err(e),
    }
}

pub(crate) fn rotate(dir: &Path) -> io::Result<String> {
    let token = generate_token()?;
    secure_fs::write_private(&dir.join(TOKEN_FILE), token.as_bytes())?;
    Ok(token)
}

/// Constant-time comparison.
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

pub(crate) struct Guard {
    pub(crate) token: RwLock<String>,
}

/// Rejects browser requests (403) and requests without the bearer token (401).
pub(crate) async fn guard(State(g): State<Arc<Guard>>, req: Request, next: Next) -> Response {
    let headers = req.headers();
    // Browsers, including the app's own webview, send `Origin` on POST and
    // `Sec-Fetch-Site` on everything; MCP clients are not browsers.
    let from_browser = headers.contains_key(header::ORIGIN)
        || headers.get("sec-fetch-site").is_some_and(|v| v.as_bytes() != b"none");
    if from_browser {
        return finish((StatusCode::FORBIDDEN, "browser requests are not allowed").into_response());
    }

    let presented = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer ").or_else(|| v.strip_prefix("bearer ")))
        .map(str::trim);
    let ok = presented.is_some_and(|p| ct_eq(p.as_bytes(), g.token.read().unwrap().as_bytes()));
    if !ok {
        let mut resp = (StatusCode::UNAUTHORIZED, "missing or invalid bearer token").into_response();
        resp.headers_mut().insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
        return finish(resp);
    }
    finish(next.run(req).await)
}

fn finish(mut resp: Response) -> Response {
    let h = resp.headers_mut();
    h.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens() {
        let a = generate_token().unwrap();
        assert!(is_well_formed(&a), "{a}");
        assert_ne!(a, generate_token().unwrap());
        assert!(ct_eq(a.as_bytes(), a.clone().as_bytes()));
        assert!(!ct_eq(a.as_bytes(), b"short"));

        let dir = std::env::temp_dir().join(format!("mcp-auth-{}", std::process::id()));
        secure_fs::create_private_dir(&dir).unwrap();
        let t1 = load_or_create(&dir).unwrap();
        assert_eq!(load_or_create(&dir).unwrap(), t1, "persists");
        let t2 = rotate(&dir).unwrap();
        assert_ne!(t1, t2);
        assert_eq!(load_or_create(&dir).unwrap(), t2);
        fs::write(dir.join(TOKEN_FILE), "garbage").unwrap();
        assert_ne!(load_or_create(&dir).unwrap(), "garbage");
        fs::remove_dir_all(&dir).unwrap();
    }
}
