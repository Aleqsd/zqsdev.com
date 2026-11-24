//! Configuration from Worker vars and secrets, read through a lookup function so that the same
//! parsing runs in the Worker, in the Durable Object, in native tests and against the committed
//! `api/wrangler.jsonc`.

use std::fmt;

use super::auth;
use super::logline::Level;
use super::money::{limit_nusd, Rates};
use super::reply;

/// Model used when `OPENAI_MODEL` is not set.
pub const DEFAULT_MODEL: &str = "gpt-6-luna";
/// The only remote base URL the OpenAI key may be sent to.
pub const OPENAI_BASE_URL: &str = "https://api.openai.com/v1";

/// Price vars in the order of `Rates`: input, cached input, cache write, output (USD per 1M tokens).
pub const PRICE_VARS: [&str; 4] = [
    "AI_INPUT_USD_PER_MILLION",
    "AI_CACHED_USD_PER_MILLION",
    "AI_CACHE_WRITE_USD_PER_MILLION",
    "AI_OUTPUT_USD_PER_MILLION",
];

/// Budget vars and defaults, in window order: minute, hour, day, month.
pub const BUDGET_VARS: [(&str, f64); 4] = [
    ("AI_MINUTE_BUDGET_USD", 0.5),
    ("AI_HOUR_BUDGET_USD", 2.0),
    ("AI_DAY_BUDGET_USD", 2.0),
    ("AI_MONTH_BUDGET_USD", 10.0),
];

pub const INVALID_PRICE: &str = "Invalid model price";
pub const MISSING_PRICES: &str = "A custom model requires explicit token prices";
pub const INVALID_BASE_URL: &str = "Invalid OPENAI_BASE_URL";

/// Built-in prices (USD per 1M tokens), exact model string match.
pub fn builtin_prices(model: &str) -> Option<[f64; 4]> {
    match model {
        "gpt-6-luna" => Some([0.10, 0.01, 0.125, 0.50]),
        "gpt-5.6-luna" => Some([0.20, 0.02, 0.25, 1.20]),
        _ => None,
    }
}

/// Timeouts and the `/api/ai` time budget, in milliseconds (design 1.6). Tests shrink them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timing {
    /// The whole `/api/ai` budget from the start of handling (admission included).
    pub deadline_ms: u64,
    /// Kept between the provider timer and the deadline.
    pub margin_ms: u64,
    /// The provider call never gets more than this (reqwest's 15 s total timeout).
    pub provider_cap_ms: u64,
    pub admit_timeout_ms: u64,
    pub finish_timeout_ms: u64,
    /// Delay before the single settlement retry in `wait_until`.
    pub retry_delay_ms: u64,
    pub retry_timeout_ms: u64,
    pub command_timeout_ms: u64,
    pub ping_timeout_ms: u64,
    pub admin_timeout_ms: u64,
}

impl Default for Timing {
    fn default() -> Timing {
        Timing {
            deadline_ms: 16_000,
            margin_ms: 500,
            provider_cap_ms: 15_000,
            admit_timeout_ms: 2_000,
            finish_timeout_ms: 1_500,
            retry_delay_ms: 1_000,
            retry_timeout_ms: 2_000,
            command_timeout_ms: 2_000,
            ping_timeout_ms: 2_000,
            admin_timeout_ms: 5_000,
        }
    }
}

/// Worker configuration, built once per isolate.
#[derive(Clone)]
pub struct Config {
    /// `OPENAI_MODEL`: sent to OpenAI and reported by `/api/version`.
    pub model: String,
    /// Token prices; zero when the price configuration is invalid.
    pub rates: Rates,
    /// `{base}/chat/completions`.
    pub endpoint: String,
    /// `OPENAI_API_KEY` when present and not blank.
    pub openai_key: Option<String>,
    /// `ADMIN_TOKEN` when it has at least 32 characters (admin routes enabled).
    pub admin_token: Option<String>,
    /// Key present, prices valid and base URL allowed.
    pub ai_configured: bool,
    /// Why AI is disabled despite a key (`⚙️ Invalid AI configuration; AI disabled`).
    pub ai_error: Option<&'static str>,
    pub log_level: Level,
    pub timing: Timing,
    /// `/api/version` body, built once.
    pub version_body: String,
}

impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config")
            .field("model", &self.model)
            .field("rates", &self.rates)
            .field("endpoint", &self.endpoint)
            .field(
                "openai_key",
                &self.openai_key.as_ref().map(|_| "[redacted]"),
            )
            .field(
                "admin_token",
                &self.admin_token.as_ref().map(|_| "[redacted]"),
            )
            .field("ai_configured", &self.ai_configured)
            .field("ai_error", &self.ai_error)
            .field("log_level", &self.log_level)
            .field("timing", &self.timing)
            .finish()
    }
}

impl Config {
    /// Reads `OPENAI_API_KEY`, `ADMIN_TOKEN`, `OPENAI_MODEL`, the four price vars,
    /// `OPENAI_BASE_URL` and `LOG_LEVEL`. Never fails: an invalid price or base URL disables AI.
    pub fn from_lookup(get: &dyn Fn(&str) -> Option<String>) -> Config {
        let model = get("OPENAI_MODEL").unwrap_or_else(|| DEFAULT_MODEL.to_string());
        let openai_key = get("OPENAI_API_KEY").filter(|k| !k.trim().is_empty());
        let admin_token = auth::usable_token(get("ADMIN_TOKEN"));
        let log_level = get("LOG_LEVEL").map_or(Level::Info, |v| Level::parse(&v));

        let (rates, mut ai_error) = match prices(&model, get) {
            Ok(rates) => (rates, None),
            Err(error) => (Rates::default(), Some(error)),
        };
        let base = get("OPENAI_BASE_URL").unwrap_or_else(|| OPENAI_BASE_URL.to_string());
        if !allowed_base_url(&base) {
            ai_error = ai_error.or(Some(INVALID_BASE_URL));
        }
        let ai_configured = openai_key.is_some() && ai_error.is_none();
        let version_body = reply::version_json(ai_configured, &model);
        Config {
            endpoint: format!("{base}/chat/completions"),
            model,
            rates,
            openai_key,
            admin_token,
            ai_configured,
            ai_error,
            log_level,
            timing: Timing::default(),
            version_body,
        }
    }
}

/// The original price rules: each set var overrides its default (parsed with `f64::from_str`, no
/// trimming); a model outside the table needs all four; each price must be finite, ≥ 0 and
/// ≤ $1,000,000 per million tokens.
fn prices(model: &str, get: &dyn Fn(&str) -> Option<String>) -> Result<Rates, &'static str> {
    let defaults = builtin_prices(model);
    let mut usd = [0.0f64; 4];
    for (slot, (i, name)) in usd.iter_mut().zip(PRICE_VARS.iter().enumerate()) {
        *slot = match get(name) {
            Some(value) => value.parse::<f64>().map_err(|_| INVALID_PRICE)?,
            None => defaults
                .and_then(|d| d.get(i).copied())
                .ok_or(MISSING_PRICES)?,
        };
    }
    Rates::from_usd_per_million(usd).ok_or(INVALID_PRICE)
}

/// `https://api.openai.com/v1` exactly, or the local stub at `http://127.0.0.1:<port>/v1` or
/// `http://localhost:<port>/v1`. Anything else would send the bearer key elsewhere.
pub fn allowed_base_url(base: &str) -> bool {
    if base == OPENAI_BASE_URL {
        return true;
    }
    let Some(rest) = base
        .strip_prefix("http://127.0.0.1:")
        .or_else(|| base.strip_prefix("http://localhost:"))
    else {
        return false;
    };
    let Some(port) = rest.strip_suffix("/v1") else {
        return false;
    };
    !port.is_empty()
        && port.len() <= 5
        && port.bytes().all(|b| b.is_ascii_digit())
        && port.parse::<u16>().is_ok_and(|p| p > 0)
}

/// Durable Object configuration (spend limits, caps, retention, text budget).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LedgerConfig {
    /// Window limits in nano-USD (minute, hour, day, month); `None` when a budget is negative or
    /// not finite: every admission is then denied with `budget_unavailable`.
    pub limits: Option<[i64; 4]>,
    /// The parsed budgets in USD, for `stats`.
    pub limits_usd: [f64; 4],
    /// `AI_DAILY_MAX_CALLS`: admitted AI calls per UTC day.
    pub daily_max_calls: i64,
    /// `COMMAND_LOG_DAILY_CAP`: stored command lines per UTC day.
    pub command_daily_cap: i64,
    /// `LOG_RETENTION_DAYS`: 0 = never delete questions or commands.
    pub retention_days: i64,
    /// `LOG_TEXT_BUDGET_MB` in bytes.
    pub text_budget_bytes: i64,
    pub log_level: Level,
}

pub const DEFAULT_DAILY_MAX_CALLS: i64 = 2_000;
pub const DEFAULT_COMMAND_DAILY_CAP: i64 = 10_000;
pub const DEFAULT_TEXT_BUDGET_MB: i64 = 256;
const MIB: i64 = 1_048_576;

impl LedgerConfig {
    pub fn from_lookup(get: &dyn Fn(&str) -> Option<String>) -> LedgerConfig {
        // Unparseable budgets fall back to their default silently (original `main.rs:79-90`).
        let mut limits_usd = [0.0f64; 4];
        for (slot, (name, default)) in limits_usd.iter_mut().zip(BUDGET_VARS) {
            *slot = get(name)
                .and_then(|v| v.parse::<f64>().ok())
                .unwrap_or(default);
        }
        let [m, h, d, mo] = limits_usd.map(limit_nusd);
        let limits = match (m, h, d, mo) {
            (Some(m), Some(h), Some(d), Some(mo)) => Some([m, h, d, mo]),
            _ => None,
        };
        let int = |name: &str| get(name).and_then(|v| v.parse::<i64>().ok());
        let retention = int("LOG_RETENTION_DAYS").unwrap_or(0);
        LedgerConfig {
            limits,
            limits_usd,
            daily_max_calls: int("AI_DAILY_MAX_CALLS")
                .unwrap_or(DEFAULT_DAILY_MAX_CALLS)
                .clamp(1, 100_000),
            command_daily_cap: int("COMMAND_LOG_DAILY_CAP")
                .unwrap_or(DEFAULT_COMMAND_DAILY_CAP)
                .clamp(120, 1_000_000),
            // 0, negative or unparseable: keep everything (the original never deleted logs).
            retention_days: if retention <= 0 {
                0
            } else {
                retention.clamp(31, 3_650)
            },
            text_budget_bytes: int("LOG_TEXT_BUDGET_MB")
                .unwrap_or(DEFAULT_TEXT_BUDGET_MB)
                .clamp(1, 768)
                * MIB,
            log_level: get("LOG_LEVEL").map_or(Level::Info, |v| Level::parse(&v)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn lookup(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |name: &str| map.get(name).cloned()
    }

    fn config(pairs: &[(&str, &str)]) -> Config {
        Config::from_lookup(&lookup(pairs))
    }

    const KEY: (&str, &str) = ("OPENAI_API_KEY", "sk-test");

    #[test]
    fn price_table_and_overrides() {
        let c = config(&[KEY]);
        assert_eq!(c.model, "gpt-6-luna");
        assert!(c.ai_configured);
        assert_eq!(c.ai_error, None);
        assert_eq!(
            c.rates,
            Rates::from_usd_per_million([0.10, 0.01, 0.125, 0.50]).unwrap()
        );
        assert_eq!(c.endpoint, "https://api.openai.com/v1/chat/completions");
        let c = config(&[KEY, ("OPENAI_MODEL", "gpt-5.6-luna")]);
        assert_eq!(
            c.rates,
            Rates::from_usd_per_million([0.20, 0.02, 0.25, 1.20]).unwrap()
        );
        // One var overrides only its own default.
        let c = config(&[KEY, ("AI_OUTPUT_USD_PER_MILLION", "2")]);
        assert_eq!(c.rates.output, 2_000_000);
        assert_eq!(c.rates.input, 100_000);
        // A custom model needs all four prices.
        let c = config(&[KEY, ("OPENAI_MODEL", "gpt-6.1-terra")]);
        assert!(!c.ai_configured);
        assert_eq!(c.ai_error, Some(MISSING_PRICES));
        let all = [
            KEY,
            ("OPENAI_MODEL", "custom"),
            ("AI_INPUT_USD_PER_MILLION", "1"),
            ("AI_CACHED_USD_PER_MILLION", "0.1"),
            ("AI_CACHE_WRITE_USD_PER_MILLION", "1.25"),
            ("AI_OUTPUT_USD_PER_MILLION", "4"),
        ];
        let c = config(&all);
        assert!(c.ai_configured);
        assert_eq!(c.rates.write, 1_250_000);
        assert_eq!(&c.version_body[..19], r#"{"ai_configured":tr"#);
    }

    #[test]
    fn invalid_configuration_disables_ai() {
        for bad in ["abc", "-1", "NaN", "inf", "1e12", " 0.1", ""] {
            let c = config(&[KEY, ("AI_INPUT_USD_PER_MILLION", bad)]);
            assert!(!c.ai_configured, "{bad:?}");
            assert_eq!(c.ai_error, Some(INVALID_PRICE), "{bad:?}");
            assert!(c.version_body.contains(r#""ai_configured":false"#));
        }
        let c = config(&[KEY, ("AI_INPUT_USD_PER_MILLION", "1000000")]);
        assert!(c.ai_configured);
        let c = config(&[("OPENAI_API_KEY", "   ")]);
        assert!(!c.ai_configured);
        assert_eq!(c.ai_error, None);
        assert!(c.openai_key.is_none());
        let c = config(&[]);
        assert!(!c.ai_configured);
    }

    #[test]
    fn base_url_rule() {
        for ok in [
            "https://api.openai.com/v1",
            "http://127.0.0.1:8733/v1",
            "http://localhost:9911/v1",
        ] {
            assert!(allowed_base_url(ok), "{ok}");
        }
        for bad in [
            "https://api.openai.com/v1/",
            "https://evil.example/v1",
            "http://127.0.0.1/v1",
            "http://127.0.0.1:0/v1",
            "http://127.0.0.1:99999/v1",
            "http://127.0.0.1:80/v2",
            "http://127.0.0.1:80@evil.example/v1",
            "http://localhost.evil.example:80/v1",
            "https://api.openai.com/v1?x",
        ] {
            assert!(!allowed_base_url(bad), "{bad}");
        }
        let c = config(&[KEY, ("OPENAI_BASE_URL", "http://127.0.0.1:8733/v1")]);
        assert!(c.ai_configured);
        assert_eq!(c.endpoint, "http://127.0.0.1:8733/v1/chat/completions");
        let c = config(&[KEY, ("OPENAI_BASE_URL", "https://proxy.example/v1")]);
        assert!(!c.ai_configured);
        assert_eq!(c.ai_error, Some(INVALID_BASE_URL));
    }

    #[test]
    fn admin_token_and_log_level() {
        let c = config(&[("ADMIN_TOKEN", &"a".repeat(31))]);
        assert!(c.admin_token.is_none());
        let c = config(&[("ADMIN_TOKEN", &"a".repeat(43)), ("LOG_LEVEL", "debug")]);
        assert_eq!(c.admin_token.as_deref(), Some("a".repeat(43).as_str()));
        assert_eq!(c.log_level, Level::Debug);
        let debug = format!("{c:?}");
        assert!(!debug.contains(&"a".repeat(43)));
    }

    #[test]
    fn ledger_budgets_caps_and_clamps() {
        let l = LedgerConfig::from_lookup(&lookup(&[]));
        assert_eq!(
            l.limits,
            Some([500_000_000, 2_000_000_000, 2_000_000_000, 10_000_000_000])
        );
        assert_eq!(l.daily_max_calls, 2_000);
        assert_eq!(l.command_daily_cap, 10_000);
        assert_eq!(l.retention_days, 0);
        assert_eq!(l.text_budget_bytes, 256 * 1_048_576);
        // Unparseable budget → default; negative or non-finite → invalid.
        let l = LedgerConfig::from_lookup(&lookup(&[("AI_HOUR_BUDGET_USD", "two")]));
        assert_eq!(l.limits.unwrap()[1], 2_000_000_000);
        for bad in ["-0.01", "inf", "NaN"] {
            let l = LedgerConfig::from_lookup(&lookup(&[("AI_DAY_BUDGET_USD", bad)]));
            assert_eq!(l.limits, None, "{bad}");
        }
        let l = LedgerConfig::from_lookup(&lookup(&[("AI_MONTH_BUDGET_USD", "5e9")]));
        assert_eq!(l.limits.unwrap()[3], 1_000_000_000_000_000_000);
        let l = LedgerConfig::from_lookup(&lookup(&[
            ("AI_DAILY_MAX_CALLS", "0"),
            ("COMMAND_LOG_DAILY_CAP", "5"),
            ("LOG_RETENTION_DAYS", "7"),
            ("LOG_TEXT_BUDGET_MB", "100000"),
        ]));
        assert_eq!(l.daily_max_calls, 1);
        assert_eq!(l.command_daily_cap, 120);
        assert_eq!(l.retention_days, 31);
        assert_eq!(l.text_budget_bytes, 768 * 1_048_576);
        let l = LedgerConfig::from_lookup(&lookup(&[
            ("AI_DAILY_MAX_CALLS", "1e3"),
            ("COMMAND_LOG_DAILY_CAP", "99999999"),
            ("LOG_RETENTION_DAYS", "-5"),
            ("LOG_TEXT_BUDGET_MB", "0"),
        ]));
        assert_eq!(l.daily_max_calls, 2_000);
        assert_eq!(l.command_daily_cap, 1_000_000);
        assert_eq!(l.retention_days, 0);
        assert_eq!(l.text_budget_bytes, 1_048_576);
        let l = LedgerConfig::from_lookup(&lookup(&[("LOG_RETENTION_DAYS", "9999")]));
        assert_eq!(l.retention_days, 3_650);
    }

    /// JSONC as wrangler reads it: `//` and `/* */` comments outside strings, trailing commas.
    fn strip_jsonc(text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        let mut chars = text.chars().peekable();
        let mut in_string = false;
        while let Some(c) = chars.next() {
            if in_string {
                out.push(c);
                if c == '\\' {
                    if let Some(next) = chars.next() {
                        out.push(next);
                    }
                } else if c == '"' {
                    in_string = false;
                }
                continue;
            }
            match (c, chars.peek()) {
                ('"', _) => {
                    in_string = true;
                    out.push(c);
                }
                ('/', Some('/')) => {
                    while chars.peek().is_some_and(|next| *next != '\n') {
                        chars.next();
                    }
                }
                ('/', Some('*')) => {
                    chars.next();
                    let mut previous = ' ';
                    for next in chars.by_ref() {
                        if previous == '*' && next == '/' {
                            break;
                        }
                        previous = next;
                    }
                }
                _ => out.push(c),
            }
        }
        // Trailing commas: a comma followed only by whitespace before `}` or `]`.
        let mut result = String::with_capacity(out.len());
        let mut in_string = false;
        let bytes: Vec<char> = out.chars().collect();
        let mut i = 0;
        while i < bytes.len() {
            let c = bytes[i];
            if in_string {
                result.push(c);
                if c == '\\' && i + 1 < bytes.len() {
                    result.push(bytes[i + 1]);
                    i += 1;
                } else if c == '"' {
                    in_string = false;
                }
            } else if c == '"' {
                in_string = true;
                result.push(c);
            } else if c == ',' {
                let rest = bytes[i + 1..].iter().find(|ch| !ch.is_whitespace());
                if !matches!(rest, Some('}') | Some(']')) {
                    result.push(c);
                }
            } else {
                result.push(c);
            }
            i += 1;
        }
        result
    }

    #[test]
    fn jsonc_stripping() {
        let text = "{\n  // comment \"x\"\n  \"a\": \"b // not a comment\", /* block\n */ \"c\": [1, 2,],\n  \"d\": \"q\\\"/*\",\n}\n";
        let value: serde_json::Value = serde_json::from_str(&strip_jsonc(text)).unwrap();
        assert_eq!(
            value,
            serde_json::json!({"a": "b // not a comment", "c": [1, 2], "d": "q\"/*"})
        );
    }

    /// The committed `api/wrangler.jsonc` must give a working AI configuration and the intended
    /// limits: `make deploy-api` runs this test first, so an invalid price, budget or cap fails
    /// the deploy instead of disabling AI in production.
    #[test]
    fn production_config_is_valid() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("wrangler.jsonc");
        let text = std::fs::read_to_string(&path).unwrap();
        let config: serde_json::Value = serde_json::from_str(&strip_jsonc(&text)).unwrap();
        assert_eq!(config["name"], "zqsdev-api");
        assert_eq!(config["main"], "build/index.js");
        let date = config["compatibility_date"].as_str().unwrap();
        assert!(
            date <= "2026-10-30",
            "wrangler 4.156.0's local workerd refuses later compatibility dates: {date}"
        );
        assert_eq!(config["durable_objects"]["bindings"][0]["name"], "LEDGER");
        assert_eq!(
            config["durable_objects"]["bindings"][0]["class_name"],
            "Ledger"
        );
        assert_eq!(config["exports"]["Ledger"]["type"], "durable-object");
        assert_eq!(config["exports"]["Ledger"]["storage"], "sqlite");
        let required: Vec<&str> = config["secrets"]["required"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(required, ["OPENAI_API_KEY", "ADMIN_TOKEN"]);

        let vars: HashMap<String, String> = config["vars"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(name, value)| {
                let text = value.as_str();
                assert!(text.is_some(), "var {name} must be a string");
                (name.clone(), text.unwrap().to_string())
            })
            .collect();
        for secret in ["OPENAI_API_KEY", "ADMIN_TOKEN"] {
            assert!(
                !vars.contains_key(secret),
                "{secret} is a secret, never a var"
            );
        }
        assert!(
            !vars.contains_key("OPENAI_BASE_URL"),
            "OPENAI_BASE_URL only points local tests at the stub"
        );
        let lookup = |name: &str| match name {
            "OPENAI_API_KEY" => Some("sk-production-config-check".to_string()),
            "ADMIN_TOKEN" => Some("a".repeat(43)),
            _ => vars.get(name).cloned(),
        };

        let cfg = Config::from_lookup(&lookup);
        assert!(cfg.ai_configured, "AI disabled: {:?}", cfg.ai_error);
        assert_eq!(cfg.model, vars["OPENAI_MODEL"]);
        assert_eq!(cfg.endpoint, "https://api.openai.com/v1/chat/completions");
        assert!(cfg.admin_token.is_some());
        assert!(["error", "warn", "info", "debug"].contains(&vars["LOG_LEVEL"].as_str()));

        // Budgets and caps must parse as written: an unparseable value would silently fall
        // back to its default, and an out-of-range one would be clamped.
        for (name, _) in BUDGET_VARS {
            let usd: f64 = vars[name].parse().unwrap();
            assert!(usd.is_finite() && usd >= 0.0, "{name}");
        }
        let ledger = LedgerConfig::from_lookup(&lookup);
        assert!(ledger.limits.is_some());
        let int = |name: &str| -> i64 { vars[name].parse().unwrap() };
        assert_eq!(ledger.daily_max_calls, int("AI_DAILY_MAX_CALLS"));
        assert_eq!(ledger.command_daily_cap, int("COMMAND_LOG_DAILY_CAP"));
        assert_eq!(ledger.retention_days, int("LOG_RETENTION_DAYS"));
        assert_eq!(
            ledger.text_budget_bytes,
            int("LOG_TEXT_BUDGET_MB") * 1_048_576
        );
        assert_eq!(ledger.log_level, cfg.log_level);
    }
}
