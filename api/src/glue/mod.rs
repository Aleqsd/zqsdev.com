//! Thin workers-rs adapters (wasm32 only): the fetch handler, the `Ledger` Durable Object,
//! `transactionSync`, the SQL store, the platform (Durable Object stub, OpenAI fetch,
//! `crypto.randomUUID()`, `Date.now()`), per-isolate configuration and console logs.

mod config;
mod input;
mod ledger_do;
mod log;
mod platform;
mod sql_store;
mod txn;

use std::time::Duration;

use serde_json::json;
use worker::{event, Context, Delay, Env, Request};

use self::input::RequestParts;
use self::platform::WorkerPlatform;
use crate::core::api::{handle, retry_finish};
use crate::core::http::{wants_body, Body, In, BODY_CAP};
use crate::core::logline::Level;
use crate::core::platform::Platform;

#[event(fetch)]
async fn fetch(mut req: Request, env: Env, ctx: Context) -> worker::Result<web_sys::Response> {
    let cfg = config::config(&env);
    let parts = RequestParts::of(&req);
    let platform = WorkerPlatform::new(env, cfg.clone(), &req);
    // Only routes that parse a body read it, and only once the content type is acceptable
    // (axum's extractor order).
    let body = if wants_body(&parts.method, &parts.path, parts.content_type.as_deref()) {
        input::read_capped(&mut req, parts.content_length.as_deref(), BODY_CAP).await
    } else {
        Body::Empty
    };
    let request = In {
        method: &parts.method,
        path: &parts.path,
        content_type: parts.content_type.as_deref(),
        content_length: parts.content_length.as_deref(),
        cf_connecting_ip: parts.cf_connecting_ip.as_deref(),
        body,
    };
    let mut out = handle(&platform, &cfg, request).await;

    // A settlement that failed inline is retried once, 1 s later, after the response.
    if let Some(retry) = out.retry.take() {
        let retry_platform = platform.clone();
        let retry_cfg = cfg.clone();
        ctx.wait_until(async move {
            Delay::from(Duration::from_millis(retry_cfg.timing.retry_delay_ms)).await;
            retry_finish(&retry_platform, &retry_cfg, &retry).await;
        });
    }

    match input::to_response(&out) {
        Ok(response) => Ok(response),
        Err(error) => {
            platform.log(
                Level::Error,
                "🔥 Unexpected internal error",
                json!({ "route": parts.path, "error": log::js_error(&error) }),
            );
            input::internal_error(&parts.path)
                .map_err(|error| worker::Error::RustError(log::js_error(&error)))
        }
    }
}
