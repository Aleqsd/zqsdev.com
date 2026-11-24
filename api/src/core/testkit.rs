//! Test support: a `Platform` with a virtual clock whose ledger is a real `Book` over SQLite
//! (reached through a JSON round trip, as on the wire) and whose provider is scripted.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::time::Duration;

use serde_json::{json, Value};

use super::config::{Config, LedgerConfig};
use super::http::{Body, In, Out};
use super::ledger::protocol::{LedgerOp, LedgerReply};
use super::ledger::serve::{serve, DoLog};
use super::ledger::sqlite_store::SqliteHost;
use super::logline::{Level, Throttle};
use super::platform::{Page, Platform, PlatformError, ProviderError, ProviderHttp};
use super::provider::REPLY_CAP;

/// 2026-11-24T00:00:00Z.
pub const T0_MS: u64 = 1_795_478_400_000;

/// What the next ledger call does.
#[derive(Clone, Debug)]
pub enum LedgerStep {
    /// Reaches the Durable Object after `latency_ms` (virtual), unless that exceeds the timeout.
    Normal { latency_ms: u64 },
    /// Fails before reaching the Durable Object.
    Fail(String),
    /// Answers this reply without reaching the Durable Object (for replies storage cannot produce).
    Reply(LedgerReply),
}

/// What the next provider call returns.
#[derive(Clone, Debug)]
pub struct ProviderStep {
    pub status: u16,
    pub body: Vec<u8>,
    pub latency_ms: u64,
    pub transport_error: Option<String>,
}

impl ProviderStep {
    pub fn reply(status: u16, body: Value, latency_ms: u64) -> ProviderStep {
        ProviderStep {
            status,
            body: body.to_string().into_bytes(),
            latency_ms,
            transport_error: None,
        }
    }

    pub fn transport(error: &str) -> ProviderStep {
        ProviderStep {
            status: 0,
            body: Vec::new(),
            latency_ms: 0,
            transport_error: Some(error.to_string()),
        }
    }
}

/// A successful Chat Completions reply.
pub fn openai_reply(model: &str, answer: &str, sources: &[&str], usage: Value) -> Value {
    let content = json!({ "answer": answer, "sources": sources }).to_string();
    json!({
        "model": model,
        "choices": [{ "finish_reason": "stop", "message": { "content": content } }],
        "usage": usage,
    })
}

pub struct FakePlatform {
    pub clock_ms: Cell<u64>,
    pub host: SqliteHost,
    pub ledger_cfg: Cell<LedgerConfig>,
    pub ledger_script: RefCell<VecDeque<LedgerStep>>,
    pub ledger_calls: RefCell<Vec<&'static str>>,
    pub provider_script: RefCell<VecDeque<ProviderStep>>,
    pub provider_bodies: RefCell<Vec<String>>,
    pub provider_timers: RefCell<Vec<Duration>>,
    pub logs: RefCell<Vec<(Level, &'static str, Value)>>,
    pub do_logs: RefCell<Vec<(Level, &'static str, Value)>>,
    uuids: Cell<u64>,
    throttle: Throttle,
    do_throttle: Throttle,
}

impl FakePlatform {
    pub fn new(ledger_cfg: LedgerConfig) -> FakePlatform {
        FakePlatform {
            clock_ms: Cell::new(T0_MS),
            host: SqliteHost::new(),
            ledger_cfg: Cell::new(ledger_cfg),
            ledger_script: RefCell::new(VecDeque::new()),
            ledger_calls: RefCell::new(Vec::new()),
            provider_script: RefCell::new(VecDeque::new()),
            provider_bodies: RefCell::new(Vec::new()),
            provider_timers: RefCell::new(Vec::new()),
            logs: RefCell::new(Vec::new()),
            do_logs: RefCell::new(Vec::new()),
            uuids: Cell::new(0),
            throttle: Throttle::new(),
            do_throttle: Throttle::new(),
        }
    }

    pub fn advance(&self, ms: u64) {
        self.clock_ms.set(self.clock_ms.get() + ms);
    }

    pub fn now_s(&self) -> i64 {
        (self.clock_ms.get() / 1000) as i64
    }

    pub fn script_ledger(&self, step: LedgerStep) {
        self.ledger_script.borrow_mut().push_back(step);
    }

    pub fn script_provider(&self, step: ProviderStep) {
        self.provider_script.borrow_mut().push_back(step);
    }

    pub fn provider_calls(&self) -> usize {
        self.provider_bodies.borrow().len()
    }

    pub fn logged(&self, msg: &str) -> Vec<Value> {
        self.logs
            .borrow()
            .iter()
            .filter(|(_, m, _)| *m == msg)
            .map(|(_, _, f)| f.clone())
            .collect()
    }

    /// The Durable Object side: one op body, as the glue's `fetch` handler would run it.
    pub fn serve_bytes(&self, body: &[u8]) -> super::ledger::serve::DoResponse {
        let sink = |level: Level, msg: &'static str, fields: Value| {
            self.do_logs.borrow_mut().push((level, msg, fields));
        };
        let log = DoLog {
            sink: &sink,
            throttle: &self.do_throttle,
            now_ms: self.clock_ms.get(),
        };
        serve(
            &self.host,
            self.ledger_cfg.get(),
            self.clock_ms.get(),
            body,
            &log,
        )
    }

    /// Runs an op directly on the Durable Object side (no script, no latency).
    pub fn op(&self, op: &LedgerOp) -> LedgerReply {
        let response = self.serve_bytes(&serde_json::to_vec(op).unwrap());
        serde_json::from_slice(&response.body).unwrap()
    }

    /// Applies the next scripted step: `Ok(None)` when the op reaches the Durable Object,
    /// `Ok(Some(reply))` for a scripted reply.
    fn transport(
        &self,
        op: &LedgerOp,
        timeout: Duration,
    ) -> Result<Option<LedgerReply>, PlatformError> {
        self.ledger_calls.borrow_mut().push(op.name());
        let step = self
            .ledger_script
            .borrow_mut()
            .pop_front()
            .unwrap_or(LedgerStep::Normal { latency_ms: 0 });
        match step {
            LedgerStep::Fail(error) => Err(PlatformError::Transport(error)),
            LedgerStep::Reply(reply) => Ok(Some(reply)),
            LedgerStep::Normal { latency_ms } => {
                let timeout_ms = timeout.as_millis() as u64;
                if latency_ms > timeout_ms {
                    self.advance(timeout_ms);
                    Err(PlatformError::Timeout)
                } else {
                    self.advance(latency_ms);
                    Ok(None)
                }
            }
        }
    }
}

impl Platform for FakePlatform {
    async fn ledger(&self, op: &LedgerOp, timeout: Duration) -> Result<LedgerReply, PlatformError> {
        if let Some(reply) = self.transport(op, timeout)? {
            return Ok(reply);
        }
        let response = self.serve_bytes(&serde_json::to_vec(op).unwrap());
        serde_json::from_slice(&response.body).map_err(|e| PlatformError::Transport(e.to_string()))
    }

    async fn ledger_page(&self, op: &LedgerOp, timeout: Duration) -> Result<Page, PlatformError> {
        if self.transport(op, timeout)?.is_some() {
            return Err(PlatformError::Transport(
                "scripted reply to an export".into(),
            ));
        }
        let response = self.serve_bytes(&serde_json::to_vec(op).unwrap());
        if response.content_type == super::http::NDJSON {
            Ok(Page {
                next_after: response.next_after,
                body: response.body,
            })
        } else {
            match serde_json::from_slice::<LedgerReply>(&response.body) {
                Ok(LedgerReply::Error { error }) => Err(PlatformError::Rejected(error)),
                _ => Err(PlatformError::Transport("unexpected export reply".into())),
            }
        }
    }

    async fn provider(&self, body: String, timer: Duration) -> Result<ProviderHttp, ProviderError> {
        self.provider_bodies.borrow_mut().push(body);
        self.provider_timers.borrow_mut().push(timer);
        let step = self
            .provider_script
            .borrow_mut()
            .pop_front()
            .unwrap_or_else(|| {
                ProviderStep::reply(
                    200,
                    openai_reply(
                        "gpt-6-luna",
                        "Micro Mages used Python.",
                        &["projects"],
                        json!({"prompt_tokens":100,"completion_tokens":20}),
                    ),
                    0,
                )
            });
        let timer_ms = timer.as_millis() as u64;
        if step.latency_ms > timer_ms {
            self.advance(timer_ms);
            return Err(ProviderError::Timer);
        }
        self.advance(step.latency_ms);
        if let Some(error) = step.transport_error {
            return Err(ProviderError::Transport(error));
        }
        if !(200..300).contains(&step.status) {
            return Ok(ProviderHttp {
                status: step.status,
                body: Vec::new(),
            });
        }
        if step.body.len() > REPLY_CAP {
            return Err(ProviderError::TooLarge);
        }
        Ok(ProviderHttp {
            status: step.status,
            body: step.body,
        })
    }

    fn now_ms(&self) -> u64 {
        self.clock_ms.get()
    }

    fn uuid(&self) -> String {
        let n = self.uuids.get() + 1;
        self.uuids.set(n);
        format!("00000000-0000-4000-8000-{n:012x}")
    }

    fn log(&self, level: Level, msg: &'static str, fields: Value) {
        self.logs.borrow_mut().push((level, msg, fields));
    }

    fn throttle(&self) -> &Throttle {
        &self.throttle
    }
}

/// A lookup over fixed pairs.
pub fn lookup(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
    let map: HashMap<String, String> = pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    move |name: &str| map.get(name).cloned()
}

pub const ADMIN_TOKEN: &str = "test-admin-token-0123456789abcdefghijklmnop";

/// Worker config: a key, the admin token, `extra` vars.
pub fn config(extra: &[(&str, &str)]) -> Config {
    let mut pairs = vec![("OPENAI_API_KEY", "sk-test"), ("ADMIN_TOKEN", ADMIN_TOKEN)];
    pairs.extend_from_slice(extra);
    Config::from_lookup(&lookup(&pairs))
}

pub fn ledger_config(pairs: &[(&str, &str)]) -> LedgerConfig {
    LedgerConfig::from_lookup(&lookup(pairs))
}

/// A request to `handle`.
pub struct Req {
    pub method: &'static str,
    pub path: String,
    pub content_type: Option<String>,
    pub content_length: Option<String>,
    pub ip: Option<String>,
    pub body: Body,
}

impl Req {
    pub fn get(path: &str) -> Req {
        Req {
            method: "GET",
            path: path.to_string(),
            content_type: None,
            content_length: None,
            ip: Some("203.0.113.7".into()),
            body: Body::Empty,
        }
    }

    pub fn post_json(path: &str, body: &str) -> Req {
        Req {
            method: "POST",
            path: path.to_string(),
            content_type: Some("application/json".into()),
            content_length: None,
            ip: Some("203.0.113.7".into()),
            body: Body::Bytes(body.as_bytes().to_vec()),
        }
    }

    pub fn method(mut self, method: &'static str) -> Req {
        self.method = method;
        self
    }

    pub fn ip(mut self, ip: Option<&str>) -> Req {
        self.ip = ip.map(str::to_string);
        self
    }

    pub fn content_type(mut self, ct: Option<&str>) -> Req {
        self.content_type = ct.map(str::to_string);
        self
    }

    pub fn input(&self) -> In<'_> {
        In {
            method: self.method,
            path: &self.path,
            content_type: self.content_type.as_deref(),
            content_length: self.content_length.as_deref(),
            cf_connecting_ip: self.ip.as_deref(),
            body: self.body.clone(),
        }
    }
}

/// Runs `handle` to completion.
pub async fn send(p: &FakePlatform, cfg: &Config, req: &Req) -> Out {
    super::api::handle(p, cfg, req.input()).await
}

pub fn body_text(out: &Out) -> String {
    String::from_utf8(out.body.as_bytes().to_vec()).unwrap()
}

pub fn body_json(out: &Out) -> Value {
    serde_json::from_slice(out.body.as_bytes()).unwrap()
}
