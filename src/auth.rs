use axum::http::{header, HeaderMap, HeaderValue};
use sha1::{Digest, Sha1};

const SESSION_COOKIE: &str = "lumiflow_session";

pub fn enabled() -> bool {
    password().is_some()
}

pub fn authenticate(headers: &HeaderMap) -> bool {
    let Some(password) = password() else {
        return true;
    };
    let Some(cookie) = headers.get(header::COOKIE).and_then(|value| value.to_str().ok()) else {
        return false;
    };
    cookie
        .split(';')
        .map(str::trim)
        .find_map(|value| value.strip_prefix(&format!("{SESSION_COOKIE}=")))
        .is_some_and(|token| token == session_token(&password))
}

pub fn password_matches(candidate: &str) -> bool {
    password().is_some_and(|password| password == candidate)
}

pub fn session_cookie() -> HeaderValue {
    HeaderValue::from_str(&format!(
        "{SESSION_COOKIE}={}; Path=/; HttpOnly; SameSite=Strict; Max-Age=604800",
        session_token(&password().unwrap_or_default())
    ))
    .expect("session cookie is valid")
}

pub fn clear_session_cookie() -> HeaderValue {
    HeaderValue::from_static("lumiflow_session=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0")
}

fn password() -> Option<String> {
    std::env::var("LUMIFLOW_PASSWORD")
        .ok()
        .filter(|value| !value.trim().is_empty())
}

fn session_token(password: &str) -> String {
    let mut digest = Sha1::new();
    digest.update(b"lumiflow-session-v1\0");
    digest.update(password.as_bytes());
    digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
