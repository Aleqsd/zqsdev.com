//! `WorkerPlatform`: the core's `Platform` on workers-rs. Durable Object calls raced against a
//! timer, the OpenAI call with its own abort timer and a capped streamed read,
//! `crypto.randomUUID()`, `Date.now()` and the console.

use std::rc::Rc;
use std::time::Duration;

use futures_util::future::{select, Either};
use futures_util::{pin_mut, StreamExt};
use serde_json::Value;
use worker::wasm_bindgen::{JsCast, JsValue};
use worker::{js_sys, AbortSignal, Date, Delay, Env, Fetch, Headers, Method, Request, RequestInit};

use super::log;
use crate::core::config::Config;
use crate::core::http::{APPLICATION_JSON, NDJSON};
use crate::core::ledger::protocol::{LedgerOp, LedgerReply};
use crate::core::logline::{Level, Throttle};
use crate::core::platform::{Page, Platform, PlatformError, ProviderError, ProviderHttp};
use crate::core::provider::REPLY_CAP;

/// Durable Object namespace binding of `api/wrangler.jsonc`.
pub const LEDGER_BINDING: &str = "LEDGER";
/// The single global ledger instance.
pub const LEDGER_NAME: &str = "zqs-ledger-v1";
/// Only matters when the instance is created.
pub const LEDGER_LOCATION_HINT: &str = "weur";
/// The host is ignored by the Durable Object; the URL must be absolute.
pub const LEDGER_URL: &str = "https://ledger/op";
/// Local fault injection (`ZQS_FAULT_INJECTION=1` builds only): the Durable Object fails the
/// first transaction of the request after this many writes.
#[cfg(zqs_fault_injection)]
pub const FAIL_AFTER_HEADER: &str = "X-Zqs-Fail-After";

thread_local! {
    static THROTTLE: Rc<Throttle> = Rc::new(Throttle::new());
}

/// One Durable Object reply, before decoding.
struct DoReply {
    content_type: Option<String>,
    next_after: Option<String>,
    body: Vec<u8>,
}

#[derive(Clone)]
pub struct WorkerPlatform {
    env: Env,
    cfg: Rc<Config>,
    throttle: Rc<Throttle>,
    #[cfg(zqs_fault_injection)]
    fail_after: Option<String>,
}

fn transport(error: impl std::fmt::Display) -> PlatformError {
    PlatformError::Transport(error.to_string())
}

impl WorkerPlatform {
    pub fn new(env: Env, cfg: Rc<Config>, _req: &Request) -> WorkerPlatform {
        WorkerPlatform {
            env,
            cfg,
            throttle: THROTTLE.with(Rc::clone),
            #[cfg(zqs_fault_injection)]
            fail_after: _req.headers().get(FAIL_AFTER_HEADER).ok().flatten(),
        }
    }

    fn ledger_request(&self, body: &str) -> worker::Result<Request> {
        let headers = Headers::new();
        headers.set("Content-Type", APPLICATION_JSON)?;
        #[cfg(zqs_fault_injection)]
        if let Some(writes) = &self.fail_after {
            headers.set(FAIL_AFTER_HEADER, writes)?;
        }
        let mut init = RequestInit::new();
        init.with_method(Method::Post)
            .with_headers(headers)
            .with_body(Some(JsValue::from_str(body)));
        Request::new_with_init(LEDGER_URL, &init)
    }

    /// `POST https://ledger/op` on the global instance, raced against `timeout`. The Durable
    /// Object always answers 200; anything else is a transport failure.
    async fn call(&self, op: &LedgerOp, timeout: Duration) -> Result<DoReply, PlatformError> {
        let body = serde_json::to_string(op).map_err(transport)?;
        let request = self.ledger_request(&body).map_err(transport)?;
        let stub = self
            .env
            .durable_object(LEDGER_BINDING)
            .and_then(|namespace| {
                namespace.get_by_name_with_location_hint(LEDGER_NAME, LEDGER_LOCATION_HINT)
            })
            .map_err(transport)?;
        let work = async move {
            let mut response = stub.fetch_with_request(request).await.map_err(transport)?;
            let status = response.status_code();
            if status != 200 {
                return Err(PlatformError::Transport(format!("ledger HTTP {status}")));
            }
            let content_type = response.headers().get("Content-Type").ok().flatten();
            let next_after = response.headers().get("X-Next-After").ok().flatten();
            let body = response.bytes().await.map_err(transport)?;
            Ok(DoReply {
                content_type,
                next_after,
                body,
            })
        };
        let timer = Delay::from(timeout);
        pin_mut!(work);
        pin_mut!(timer);
        match select(work, timer).await {
            Either::Left((reply, _)) => reply,
            Either::Right(_) => Err(PlatformError::Timeout),
        }
    }

    fn openai_request(&self, key: &str, body: &str) -> worker::Result<Request> {
        let headers = Headers::new();
        headers.set("Authorization", &format!("Bearer {key}"))?;
        headers.set("Content-Type", APPLICATION_JSON)?;
        let mut init = RequestInit::new();
        init.with_method(Method::Post)
            .with_headers(headers)
            .with_body(Some(JsValue::from_str(body)));
        Request::new_with_init(&self.cfg.endpoint, &init)
    }
}

impl Platform for WorkerPlatform {
    async fn ledger(&self, op: &LedgerOp, timeout: Duration) -> Result<LedgerReply, PlatformError> {
        let reply = self.call(op, timeout).await?;
        serde_json::from_slice(&reply.body)
            .map_err(|error| PlatformError::Transport(format!("undecodable ledger reply: {error}")))
    }

    async fn ledger_page(&self, op: &LedgerOp, timeout: Duration) -> Result<Page, PlatformError> {
        let reply = self.call(op, timeout).await?;
        if reply
            .content_type
            .as_deref()
            .is_some_and(|value| value.starts_with(NDJSON))
        {
            return Ok(Page {
                next_after: reply.next_after,
                body: reply.body,
            });
        }
        match serde_json::from_slice::<LedgerReply>(&reply.body) {
            Ok(LedgerReply::Error { error }) => Err(PlatformError::Rejected(error)),
            Ok(other) => Err(PlatformError::Transport(format!(
                "unexpected ledger reply: {}",
                other.status()
            ))),
            Err(error) => Err(PlatformError::Transport(format!(
                "undecodable ledger reply: {error}"
            ))),
        }
    }

    /// `POST {base}/chat/completions` with the bearer key. The timer covers headers and body; a
    /// non-2xx body is never read; a 2xx body is streamed and refused above `REPLY_CAP`.
    async fn provider(&self, body: String, timer: Duration) -> Result<ProviderHttp, ProviderError> {
        let Some(key) = self.cfg.openai_key.as_deref() else {
            return Err(ProviderError::Transport("OpenAI key missing".into()));
        };
        let request = self
            .openai_request(key, &body)
            .map_err(|error| ProviderError::Transport(error.to_string()))?;
        let controller = web_sys::AbortController::new()
            .map_err(|error| ProviderError::Transport(log::js_error(&error)))?;
        let signal = AbortSignal::from(controller.signal());
        let work = async {
            let mut response = Fetch::Request(request)
                .send_with_signal(&signal)
                .await
                .map_err(|error| ProviderError::Transport(error.to_string()))?;
            let status = response.status_code();
            if !(200..300).contains(&status) {
                return Ok(ProviderHttp {
                    status,
                    body: Vec::new(),
                });
            }
            // A 2xx without a body: the parser reports it like any other non-JSON body.
            let Ok(mut stream) = response.stream() else {
                return Ok(ProviderHttp {
                    status,
                    body: Vec::new(),
                });
            };
            let mut buf = Vec::with_capacity(8 * 1024);
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|error| ProviderError::Transport(error.to_string()))?;
                if buf.len().saturating_add(chunk.len()) > REPLY_CAP {
                    return Err(ProviderError::TooLarge);
                }
                buf.extend_from_slice(&chunk);
            }
            Ok(ProviderHttp { status, body: buf })
        };
        let delay = Delay::from(timer);
        pin_mut!(work);
        pin_mut!(delay);
        let out = match select(work, delay).await {
            Either::Left((result, _)) => result,
            Either::Right(_) => Err(ProviderError::Timer),
        };
        // Cancels a pending request or an unread body; harmless after a complete read.
        controller.abort();
        out
    }

    fn now_ms(&self) -> u64 {
        Date::now().as_millis()
    }

    fn uuid(&self) -> String {
        let global: web_sys::WorkerGlobalScope = js_sys::global().unchecked_into();
        match global.crypto() {
            Ok(crypto) => crypto.random_uuid(),
            Err(_) => fallback_uuid(),
        }
    }

    fn log(&self, level: Level, msg: &'static str, fields: Value) {
        log::emit(self.cfg.log_level, level, msg, &fields);
    }

    fn throttle(&self) -> &Throttle {
        &self.throttle
    }
}

/// A version-4 UUID from `Math.random()`, only if `crypto` were ever unavailable.
fn fallback_uuid() -> String {
    let mut value: u128 = 0;
    for _ in 0..16 {
        let byte = (js_sys::Math::random() * 256.0) as u8;
        value = (value << 8) | u128::from(byte);
    }
    value = (value & !(0xF_u128 << 76)) | (0x4_u128 << 76);
    value = (value & !(0x3_u128 << 62)) | (0x2_u128 << 62);
    format!(
        "{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
        (value >> 96) as u32,
        (value >> 80) as u16,
        (value >> 64) as u16,
        (value >> 48) as u16,
        value & 0xFFFF_FFFF_FFFF
    )
}
