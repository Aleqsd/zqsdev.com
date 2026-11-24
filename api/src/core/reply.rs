//! Response bodies: the `/api/ai` success and fallback JSON (field order of the original
//! `AiResponse`), `/api/version`, `/api/health` and admin errors.

use serde::Serialize;

use super::embedded::{COMMIT, KNOWLEDGE_MODE, KNOWLEDGE_UPDATED, VERSION};

/// The bilingual fallback answer of the original server (`main.rs:70`).
pub const FALLBACK_ANSWER: &str = "AI is temporarily unavailable. Classic commands are ready: about, experience, projects, testimonials, resume. / IA temporairement indisponible : les commandes classiques restent disponibles.";

/// Builds the fallback body for one reason code at compile time.
macro_rules! fallback_json {
    ($code:literal) => {
        concat!(
            "{\"answer\":\"AI is temporarily unavailable. Classic commands are ready: about, experience, projects, testimonials, resume. / IA temporairement indisponible : les commandes classiques restent disponibles.\",\"ai_enabled\":false,\"reason\":\"",
            $code,
            "\",\"model\":null,\"sources\":[],\"knowledge_updated\":\"",
            env!("ZQS_KNOWLEDGE_UPDATED"),
            "\"}"
        )
    };
}

/// Failure reasons of `/api/ai` with their HTTP statuses (spec §4.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    InvalidRequest,
    AiNotConfigured,
    BudgetUnavailable,
    RequestLimit,
    Busy,
    SpendingLimit,
    ProviderUnavailable,
    Timeout,
}

impl Reason {
    pub const ALL: [Reason; 8] = [
        Reason::InvalidRequest,
        Reason::AiNotConfigured,
        Reason::BudgetUnavailable,
        Reason::RequestLimit,
        Reason::Busy,
        Reason::SpendingLimit,
        Reason::ProviderUnavailable,
        Reason::Timeout,
    ];

    pub fn code(self) -> &'static str {
        match self {
            Reason::InvalidRequest => "invalid_request",
            Reason::AiNotConfigured => "ai_not_configured",
            Reason::BudgetUnavailable => "budget_unavailable",
            Reason::RequestLimit => "request_limit",
            Reason::Busy => "busy",
            Reason::SpendingLimit => "spending_limit",
            Reason::ProviderUnavailable => "provider_unavailable",
            Reason::Timeout => "timeout",
        }
    }

    pub fn status(self) -> u16 {
        match self {
            Reason::InvalidRequest => 400,
            Reason::AiNotConfigured | Reason::BudgetUnavailable | Reason::ProviderUnavailable => {
                503
            }
            Reason::RequestLimit | Reason::Busy | Reason::SpendingLimit => 429,
            Reason::Timeout => 504,
        }
    }

    /// The reasons a ledger admission can return.
    pub fn from_denial(code: &str) -> Option<Reason> {
        match code {
            "request_limit" => Some(Reason::RequestLimit),
            "busy" => Some(Reason::Busy),
            "spending_limit" => Some(Reason::SpendingLimit),
            "budget_unavailable" => Some(Reason::BudgetUnavailable),
            _ => None,
        }
    }

    /// The fallback body, a compile-time constant per reason.
    pub fn fallback_body(self) -> &'static str {
        match self {
            Reason::InvalidRequest => fallback_json!("invalid_request"),
            Reason::AiNotConfigured => fallback_json!("ai_not_configured"),
            Reason::BudgetUnavailable => fallback_json!("budget_unavailable"),
            Reason::RequestLimit => fallback_json!("request_limit"),
            Reason::Busy => fallback_json!("busy"),
            Reason::SpendingLimit => fallback_json!("spending_limit"),
            Reason::ProviderUnavailable => fallback_json!("provider_unavailable"),
            Reason::Timeout => fallback_json!("timeout"),
        }
    }
}

/// The original `AiResponse` (`main.rs:60-68`): same fields, same order.
#[derive(Serialize)]
pub struct AiResponse<'a> {
    pub answer: &'a str,
    pub ai_enabled: bool,
    pub reason: Option<&'a str>,
    pub model: Option<&'a str>,
    pub sources: &'a [String],
    pub knowledge_updated: &'a str,
}

/// `{"answer":…,"ai_enabled":true,"reason":null,"model":…,"sources":[…],"knowledge_updated":…}`
pub fn success_json(answer: &str, model: &str, sources: &[String]) -> String {
    serde_json::to_string(&AiResponse {
        answer,
        ai_enabled: true,
        reason: None,
        model: Some(model),
        sources,
        knowledge_updated: KNOWLEDGE_UPDATED,
    })
    .unwrap_or_else(|_| Reason::ProviderUnavailable.fallback_body().to_string())
}

/// `/api/version`: keys sorted, as the original `json!` produced them.
#[derive(Serialize)]
struct VersionBody<'a> {
    ai_configured: bool,
    commit: &'a str,
    knowledge_mode: &'a str,
    knowledge_updated: &'a str,
    model: &'a str,
    version: &'a str,
}

pub fn version_json(ai_configured: bool, model: &str) -> String {
    serde_json::to_string(&VersionBody {
        ai_configured,
        commit: COMMIT,
        knowledge_mode: KNOWLEDGE_MODE,
        knowledge_updated: KNOWLEDGE_UPDATED,
        model,
        version: VERSION,
    })
    .unwrap_or_default()
}

/// `/api/health`, keys in this order: `ok`, `ai_configured`, `ledger`, `storage`.
#[derive(Serialize)]
struct HealthBody<'a> {
    ok: bool,
    ai_configured: bool,
    ledger: &'a str,
    storage: &'a str,
}

pub fn health_json(ok: bool, ai_configured: bool, ledger: &str, storage: &str) -> String {
    serde_json::to_string(&HealthBody {
        ok,
        ai_configured,
        ledger,
        storage,
    })
    .unwrap_or_default()
}

/// Admin errors: `{"error":"…"}`.
pub fn error_json(code: &str) -> String {
    serde_json::to_string(&serde_json::json!({ "error": code })).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The constant fallbacks are byte-identical to what the original `fallback()` serialised.
    #[test]
    fn fallback_bodies_equal_the_serialised_struct() {
        for reason in Reason::ALL {
            let expected = serde_json::to_string(&AiResponse {
                answer: FALLBACK_ANSWER,
                ai_enabled: false,
                reason: Some(reason.code()),
                model: None,
                sources: &[],
                knowledge_updated: KNOWLEDGE_UPDATED,
            })
            .unwrap();
            assert_eq!(reason.fallback_body(), expected);
        }
        assert_eq!(FALLBACK_ANSWER.len(), 190);
        let statuses: Vec<u16> = Reason::ALL.iter().map(|r| r.status()).collect();
        assert_eq!(statuses, [400, 503, 503, 429, 429, 429, 503, 504]);
        assert_eq!(Reason::from_denial("busy"), Some(Reason::Busy));
        assert_eq!(Reason::from_denial("timeout"), None);
    }

    #[test]
    fn success_and_version_shapes() {
        let body = success_json(
            "Hi \"there\"",
            "gpt-6-luna-2026-09-22",
            &["projects".into()],
        );
        assert_eq!(
            body,
            format!(
                r#"{{"answer":"Hi \"there\"","ai_enabled":true,"reason":null,"model":"gpt-6-luna-2026-09-22","sources":["projects"],"knowledge_updated":"{KNOWLEDGE_UPDATED}"}}"#
            )
        );
        assert_eq!(
            version_json(true, "gpt-6-luna"),
            format!(
                r#"{{"ai_configured":true,"commit":"{COMMIT}","knowledge_mode":"full_context","knowledge_updated":"{KNOWLEDGE_UPDATED}","model":"gpt-6-luna","version":"{VERSION}"}}"#
            )
        );
        // Same bytes as the original json! macro, whose maps sort their keys.
        let original = serde_json::json!({"version":VERSION,"commit":COMMIT,"model":"gpt-6-luna","knowledge_updated":KNOWLEDGE_UPDATED,"knowledge_mode":"full_context","ai_configured":true});
        assert_eq!(version_json(true, "gpt-6-luna"), original.to_string());
        assert_eq!(
            health_json(false, true, "unavailable", "unknown"),
            r#"{"ok":false,"ai_configured":true,"ledger":"unavailable","storage":"unknown"}"#
        );
        assert_eq!(error_json("invalid_table"), r#"{"error":"invalid_table"}"#);
    }
}
