//! Shared HTTP helpers for the E2E tests, wrapping ureq 3. Non-2xx
//! responses are returned as ordinary responses (not errors): these tests
//! assert on 4xx statuses and read their JSON error bodies.
#![allow(dead_code)]

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .build()
        .into()
}

/// POSTs `body` as JSON; returns `(status, response JSON)` (`Null` when the
/// response body is not JSON).
pub fn post_json(url: &str, body: &serde_json::Value) -> (u16, serde_json::Value) {
    let mut resp = agent().post(url).send_json(body).unwrap();
    let status = resp.status().as_u16();
    let json = resp
        .body_mut()
        .read_json()
        .unwrap_or(serde_json::Value::Null);
    (status, json)
}

/// POSTs an empty body; returns `(status, response JSON)`.
pub fn post_empty(url: &str) -> (u16, serde_json::Value) {
    let mut resp = agent().post(url).send_empty().unwrap();
    let status = resp.status().as_u16();
    let json = resp
        .body_mut()
        .read_json()
        .unwrap_or(serde_json::Value::Null);
    (status, json)
}

/// GETs `url`; returns `(status, content-type, body text)`.
pub fn get_text(url: &str) -> (u16, String, String) {
    let mut resp = agent().get(url).call().unwrap();
    let status = resp.status().as_u16();
    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let body = resp.body_mut().read_to_string().unwrap();
    (status, content_type, body)
}

/// GETs `url`; returns `(status, response JSON)`.
pub fn get_json(url: &str) -> (u16, serde_json::Value) {
    let mut resp = agent().get(url).call().unwrap();
    let status = resp.status().as_u16();
    let json = resp
        .body_mut()
        .read_json()
        .unwrap_or(serde_json::Value::Null);
    (status, json)
}
