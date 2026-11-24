//! `worker::Request` → `core::http::In` (headers, capped streaming body) and
//! `core::http::Out` → `web_sys::Response`, built with fallible calls only.

use futures_util::StreamExt;
use worker::wasm_bindgen::JsValue;
use worker::{js_sys, Request};

use crate::core::http::{Body, Out, OutBody};
use crate::core::reply::Reason;

/// The request fields `core::http::In` borrows, owned by the fetch handler.
pub struct RequestParts {
    /// The method as received (not normalised: an unknown method must still get 405 or 404).
    pub method: String,
    /// Percent-encoded path, without the query string.
    pub path: String,
    pub content_type: Option<String>,
    pub content_length: Option<String>,
    pub cf_connecting_ip: Option<String>,
}

impl RequestParts {
    pub fn of(req: &Request) -> RequestParts {
        let header = |name: &str| req.headers().get(name).ok().flatten();
        RequestParts {
            method: req.inner().method(),
            path: req.path(),
            content_type: header("Content-Type"),
            content_length: header("Content-Length"),
            cf_connecting_ip: header("CF-Connecting-IP"),
        }
    }
}

/// Reads the body as a stream, stopping as soon as it exceeds `cap` bytes. A declared
/// `Content-Length` above the cap is refused before reading anything.
pub async fn read_capped(req: &mut Request, content_length: Option<&str>, cap: usize) -> Body {
    let limit = u64::try_from(cap).unwrap_or(u64::MAX);
    if content_length
        .and_then(|value| value.trim().parse::<u64>().ok())
        .is_some_and(|declared| declared > limit)
    {
        return Body::TooLarge;
    }
    // No body (GET, HEAD, an empty POST): the JSON parser then reports EOF, as axum does.
    let Ok(mut stream) = req.stream() else {
        return Body::Empty;
    };
    let mut buf = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = match chunk {
            Ok(chunk) => chunk,
            Err(error) => return Body::Unreadable(error.to_string()),
        };
        if buf.len().saturating_add(chunk.len()) > cap {
            return Body::TooLarge;
        }
        buf.extend_from_slice(&chunk);
    }
    Body::Bytes(buf)
}

/// Statuses whose responses must not carry a body (the Response constructor throws otherwise).
fn null_body_status(status: u16) -> bool {
    matches!(status, 101 | 204 | 205 | 304)
}

/// Builds the runtime response: status, `Content-Type`, the extra headers, the body.
pub fn to_response(out: &Out) -> Result<web_sys::Response, JsValue> {
    let headers = web_sys::Headers::new()?;
    if let Some(content_type) = out.content_type {
        headers.set("Content-Type", content_type)?;
    }
    for (name, value) in &out.headers {
        headers.set(name, value)?;
    }
    let init = web_sys::ResponseInit::new();
    init.set_status(out.status);
    init.set_headers_headers(&headers);
    if matches!(out.body, OutBody::Empty) || null_body_status(out.status) {
        return web_sys::Response::new_with_opt_str_and_init(None, &init);
    }
    let body = js_sys::Uint8Array::from(out.body.as_bytes());
    web_sys::Response::new_with_opt_buffer_source_and_init(Some(&body), &init)
}

/// The response when the glue itself failed: the `budget_unavailable` fallback on `/api/ai`
/// (the frontend expects JSON there), an empty 500 elsewhere.
pub fn internal_error(path: &str) -> Result<web_sys::Response, JsValue> {
    let out = if path == "/api/ai" {
        Out::json_static(
            Reason::BudgetUnavailable.status(),
            Reason::BudgetUnavailable.fallback_body(),
        )
    } else {
        Out::empty(500)
    };
    to_response(&out)
}
