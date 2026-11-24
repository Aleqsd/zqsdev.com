//! What the core needs from the runtime. The glue implements it with workers-rs (Durable Object
//! stub, `fetch` with an abort timer, `crypto.randomUUID()`, `Date.now()`, console); tests use a
//! fake with a virtual clock.

use std::fmt;
use std::time::Duration;

use serde_json::Value;

use super::ledger::protocol::{LedgerOp, LedgerReply};
use super::logline::{with_suppressed, Level, Throttle};

#[allow(async_fn_in_trait)]
pub trait Platform {
    /// One Durable Object op (`POST https://ledger/op`), raced against `timeout`.
    async fn ledger(&self, op: &LedgerOp, timeout: Duration) -> Result<LedgerReply, PlatformError>;
    /// An export op whose NDJSON reply body is passed through without parsing.
    async fn ledger_page(&self, op: &LedgerOp, timeout: Duration) -> Result<Page, PlatformError>;
    /// `POST {base}/chat/completions` with the key, aborted when `timer` fires; the reply body is
    /// read only for 2xx statuses, at most `REPLY_CAP` bytes.
    async fn provider(&self, body: String, timer: Duration) -> Result<ProviderHttp, ProviderError>;
    /// Unix time in milliseconds.
    fn now_ms(&self) -> u64;
    /// A random UUID v4.
    fn uuid(&self) -> String;
    /// One structured log line; `msg` starts with an emoji.
    fn log(&self, level: Level, msg: &'static str, fields: Value);
    /// Per-isolate throttle state for repeated warnings.
    fn throttle(&self) -> &Throttle;
}

/// Provider HTTP result. `body` is empty for non-2xx statuses (not read, as before).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderHttp {
    pub status: u16,
    pub body: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProviderError {
    /// Our timer fired (the core knows whether it was the 15 s cap or the deadline).
    Timer,
    /// A 2xx body above `REPLY_CAP`.
    TooLarge,
    Transport(String),
}

/// An export page: NDJSON lines and the cursor of the next page while more remain.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Page {
    pub next_after: Option<String>,
    pub body: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlatformError {
    /// The call did not answer in time.
    Timeout,
    /// Stub, network or decoding failure.
    Transport(String),
    /// The Durable Object answered an export with an error code instead of a page.
    Rejected(String),
}

impl fmt::Display for PlatformError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PlatformError::Timeout => f.write_str("timeout"),
            PlatformError::Transport(error) => write!(f, "transport: {error}"),
            PlatformError::Rejected(code) => write!(f, "rejected: {code}"),
        }
    }
}

/// Logs a "throttled" line: at most once per 60 s per isolate, with a `suppressed` count.
pub fn log_throttled(p: &impl Platform, level: Level, msg: &'static str, fields: Value) {
    if let Some(suppressed) = p.throttle().admit(msg, p.now_ms()) {
        p.log(level, msg, with_suppressed(fields, suppressed));
    }
}
