//! Structured log lines: one JSON object per line, `msg` always starts with an emoji, `LOG_LEVEL`
//! filters, and "throttled" lines appear at most once per 60 s per isolate with a `suppressed`
//! count. No line carries question or answer text, an IP, a token or a provider body.

use serde_json::{Map, Value};
use std::cell::RefCell;
use std::collections::HashMap;

/// Minimum interval between two lines of the same throttled message.
pub const THROTTLE_MS: u64 = 60_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Error,
    Warn,
    Info,
    Debug,
}

impl Level {
    /// `LOG_LEVEL` value: `error`, `warn`, `info` or `debug` (any case); anything else is `info`.
    pub fn parse(text: &str) -> Level {
        match text.trim().to_ascii_lowercase().as_str() {
            "error" => Level::Error,
            "warn" | "warning" => Level::Warn,
            "debug" => Level::Debug,
            _ => Level::Info,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Level::Error => "error",
            Level::Warn => "warn",
            Level::Info => "info",
            Level::Debug => "debug",
        }
    }

    /// Whether a line at `self` passes the configured filter.
    pub fn enabled(self, configured: Level) -> bool {
        self <= configured
    }
}

/// `{"level":…,"msg":…,<fields…>}`; `fields` that is not an object is stored under `detail`.
pub fn line(level: Level, msg: &str, fields: &Value) -> String {
    let mut out = String::with_capacity(96);
    out.push_str("{\"level\":\"");
    out.push_str(level.as_str());
    out.push_str("\",\"msg\":");
    out.push_str(&json_string(msg));
    let extra: Map<String, Value> = match fields {
        Value::Object(map) => map.clone(),
        Value::Null => Map::new(),
        other => {
            let mut map = Map::new();
            map.insert("detail".into(), other.clone());
            map
        }
    };
    for (key, value) in &extra {
        if key == "level" || key == "msg" {
            continue;
        }
        out.push(',');
        out.push_str(&json_string(key));
        out.push(':');
        out.push_str(&serde_json::to_string(value).unwrap_or_else(|_| "null".into()));
    }
    out.push('}');
    out
}

fn json_string(text: &str) -> String {
    serde_json::to_string(text).unwrap_or_else(|_| "\"\"".into())
}

#[derive(Clone, Copy)]
struct Slot {
    last_ms: u64,
    suppressed: u64,
}

/// Per-isolate throttle state for repeated warnings.
#[derive(Default)]
pub struct Throttle {
    slots: RefCell<HashMap<&'static str, Slot>>,
}

impl Throttle {
    pub fn new() -> Throttle {
        Throttle::default()
    }

    /// `Some(suppressed)` when `key` may be logged at `now_ms` (and resets its count), `None`
    /// when it was logged less than 60 s ago (the line is counted as suppressed).
    pub fn admit(&self, key: &'static str, now_ms: u64) -> Option<u64> {
        let Ok(mut slots) = self.slots.try_borrow_mut() else {
            return Some(0);
        };
        match slots.get_mut(key) {
            Some(slot) if now_ms.saturating_sub(slot.last_ms) < THROTTLE_MS => {
                slot.suppressed = slot.suppressed.saturating_add(1);
                None
            }
            Some(slot) => {
                let suppressed = slot.suppressed;
                *slot = Slot {
                    last_ms: now_ms,
                    suppressed: 0,
                };
                Some(suppressed)
            }
            None => {
                slots.insert(
                    key,
                    Slot {
                        last_ms: now_ms,
                        suppressed: 0,
                    },
                );
                Some(0)
            }
        }
    }
}

/// Adds `"suppressed": n` to an object of fields.
pub fn with_suppressed(fields: Value, suppressed: u64) -> Value {
    match fields {
        Value::Object(mut map) => {
            map.insert("suppressed".into(), Value::from(suppressed));
            Value::Object(map)
        }
        other => {
            let mut map = Map::new();
            if !other.is_null() {
                map.insert("detail".into(), other);
            }
            map.insert("suppressed".into(), Value::from(suppressed));
            Value::Object(map)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn levels_filter_and_parse() {
        assert_eq!(Level::parse("DEBUG"), Level::Debug);
        assert_eq!(Level::parse(" warn "), Level::Warn);
        assert_eq!(Level::parse("nonsense"), Level::Info);
        assert!(Level::Error.enabled(Level::Info));
        assert!(Level::Info.enabled(Level::Info));
        assert!(!Level::Debug.enabled(Level::Info));
        assert!(!Level::Warn.enabled(Level::Error));
    }

    #[test]
    fn line_shape() {
        assert_eq!(
            line(
                Level::Warn,
                "🧾 Ledger unavailable",
                &json!({"route":"/api/ai","error":"timeout","msg":"x"})
            ),
            r#"{"level":"warn","msg":"🧾 Ledger unavailable","error":"timeout","route":"/api/ai"}"#
        );
        assert_eq!(
            line(Level::Info, "🚀 API isolate started", &Value::Null),
            r#"{"level":"info","msg":"🚀 API isolate started"}"#
        );
    }

    #[test]
    fn throttle_once_per_minute_with_count() {
        let t = Throttle::new();
        assert_eq!(t.admit("a", 1_000), Some(0));
        assert_eq!(t.admit("a", 2_000), None);
        assert_eq!(t.admit("a", 60_999), None);
        assert_eq!(t.admit("b", 61_000), Some(0));
        assert_eq!(t.admit("a", 61_000), Some(2));
        assert_eq!(t.admit("a", 61_001), None);
        assert_eq!(
            with_suppressed(json!({"path":"/x"}), 3),
            json!({"path":"/x","suppressed":3})
        );
    }
}
