//! OpenAI Chat Completions: the request body (byte-identical to the original `json!` output) and
//! the reply checks of the original `parse()`, in the same order and with the same messages.

use serde_json::Value;

use super::embedded::{KNOWLEDGE_UPDATED, SOURCE_IDS, SYSTEM_MESSAGE_JSON};
use super::money::{usage_nusd, usage_tokens, Rates};
use super::request::Message;

/// `max_completion_tokens` of every request.
pub const MAX_OUTPUT: u32 = 640;
/// A provider reply above this size is abnormal (640 output tokens produce far less).
pub const REPLY_CAP: usize = 65_536;
/// Characters of a valid answer, before trimming.
pub const MAX_ANSWER_CHARS: usize = 5000;

/// The second system message: `Today's date: {date}. Knowledge last reviewed: {KU}.`
pub fn date_line(date: &str) -> String {
    format!("Today's date: {date}. Knowledge last reviewed: {KNOWLEDGE_UPDATED}.")
}

/// The provider body, compact with sorted keys, built without a `Value` tree: the system message
/// is pasted as a build-time escaped literal.
pub fn build_body(model: &str, date: &str, history: &[Message], question: &str) -> String {
    let history_len: usize = history.iter().map(|m| m.content.len() + 40).sum();
    let mut b = Vec::with_capacity(SYSTEM_MESSAGE_JSON.len() + 512 + question.len() + history_len);
    b.extend_from_slice(b"{\"max_completion_tokens\":640,\"messages\":[{\"content\":");
    b.extend_from_slice(SYSTEM_MESSAGE_JSON.as_bytes());
    b.extend_from_slice(b",\"role\":\"system\"},{\"content\":");
    json_str(&mut b, &date_line(date));
    b.extend_from_slice(b",\"role\":\"system\"}");
    for m in history {
        // `json!(m)` maps sort their keys: content, then role.
        b.extend_from_slice(b",{\"content\":");
        json_str(&mut b, &m.content);
        b.extend_from_slice(b",\"role\":");
        json_str(&mut b, &m.role);
        b.push(b'}');
    }
    b.extend_from_slice(b",{\"content\":");
    json_str(&mut b, question.trim());
    b.extend_from_slice(b",\"role\":\"user\"}],\"model\":");
    json_str(&mut b, model);
    b.extend_from_slice(
        b",\"reasoning_effort\":\"none\",\"response_format\":{\"type\":\"json_object\"},\"store\":false}",
    );
    // Valid UTF-8 by construction: JSON-escaped strings between ASCII framing.
    String::from_utf8(b).unwrap_or_default()
}

fn json_str(b: &mut Vec<u8>, text: &str) {
    // Writing a string into a Vec cannot fail.
    let _ = serde_json::to_writer(&mut *b, text);
}

/// The original construction (`ai.rs:141-149`), kept to prove `build_body` byte-identical.
#[cfg(test)]
pub fn build_body_reference(
    system: &str,
    model: &str,
    date: &str,
    history: &[Message],
    question: &str,
) -> String {
    use serde_json::json;
    let mut messages = vec![
        json!({"role":"system","content":system}),
        json!({"role":"system","content":date_line(date)}),
    ];
    messages.extend(history.iter().map(|m| json!(m)));
    messages.push(json!({"role":"user","content":question.trim()}));
    json!({"model":model,"reasoning_effort":"none","messages":messages,"max_completion_tokens":MAX_OUTPUT,"store":false,"response_format":{"type":"json_object"}}).to_string()
}

/// A validated answer.
#[derive(Clone, Debug, PartialEq)]
pub struct Answer {
    /// Trimmed answer text.
    pub text: String,
    /// Model reported by the provider.
    pub model: String,
    /// Known source IDs, duplicates removed in first-seen order.
    pub sources: Vec<String>,
    /// Raw `usage` object.
    pub usage: Value,
    /// Actual cost in nano-USD.
    pub nusd: i64,
}

/// Why a 2xx reply was refused. `nusd` is the actual usage cost when `usage` was valid (the call
/// is then charged that, not the reservation); `usage` is the raw object when present.
#[derive(Clone, Debug, PartialEq)]
pub struct ReplyError {
    pub class: &'static str,
    /// The original error text: one of the fixed messages, or serde_json's parse error.
    pub message: String,
    pub nusd: Option<i64>,
    pub usage: Option<Value>,
}

static NULL: Value = Value::Null;

/// `value[key]` without the panicking `Index` operator: `Null` when absent or not an object.
pub fn field<'a>(value: &'a Value, key: &str) -> &'a Value {
    value.get(key).unwrap_or(&NULL)
}

/// `value[i]`: `Null` when absent or not an array.
pub fn item(value: &Value, i: usize) -> &Value {
    value.get(i).unwrap_or(&NULL)
}

/// The original checks, in order: finish reason, content, answer, sources, usage, model.
pub fn parse_reply(body: &[u8], rates: &Rates) -> Result<Answer, ReplyError> {
    let data: Value = serde_json::from_slice(body).map_err(|_| ReplyError {
        class: "invalid_json",
        message: "error decoding response body".into(),
        nusd: None,
        usage: None,
    })?;
    let usage = field(&data, "usage");
    let raw_usage = (!usage.is_null()).then(|| usage.clone());
    let cost = usage_tokens(usage).map(|t| usage_nusd(&t, rates));
    let fail = |class: &'static str, message: String| ReplyError {
        class,
        message,
        nusd: cost.ok(),
        usage: raw_usage.clone(),
    };

    let choice = item(field(&data, "choices"), 0);
    if field(choice, "finish_reason") != "stop" {
        return Err(fail("incomplete", "Incomplete provider answer".into()));
    }
    let Some(content) = field(field(choice, "message"), "content").as_str() else {
        return Err(fail("missing_answer", "Missing answer".into()));
    };
    let result: Value =
        serde_json::from_str(content).map_err(|e| fail("invalid_json", e.to_string()))?;
    let Some(text) = field(&result, "answer")
        .as_str()
        .filter(|v| !v.trim().is_empty() && v.chars().count() <= MAX_ANSWER_CHARS)
    else {
        return Err(fail("invalid_answer", "Invalid answer".into()));
    };
    let Some(listed) = field(&result, "sources").as_array() else {
        return Err(fail("missing_sources", "Missing sources".into()));
    };
    let mut sources: Vec<String> = Vec::new();
    for value in listed {
        let Some(id) = value.as_str() else {
            return Err(fail("invalid_source", "Invalid source".into()));
        };
        if !SOURCE_IDS.contains(&id) {
            return Err(fail("unknown_source", "Unknown source ID".into()));
        }
        if !sources.iter().any(|s| s == id) {
            sources.push(id.to_string());
        }
    }
    let nusd = match cost {
        Ok(nusd) => nusd,
        Err(error) => {
            return Err(ReplyError {
                class: error.class(),
                message: error.message().into(),
                nusd: None,
                usage: raw_usage,
            })
        }
    };
    let Some(model) = field(&data, "model").as_str() else {
        return Err(fail("missing_model", "Missing model".into()));
    };
    Ok(Answer {
        text: text.trim().to_string(),
        model: model.to_string(),
        sources,
        usage: usage.clone(),
        nusd,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::money::reserve_nusd;
    use serde_json::json;

    fn luna56() -> Rates {
        Rates::from_usd_per_million([0.20, 0.02, 0.25, 1.20]).unwrap()
    }

    fn luna6() -> Rates {
        Rates::from_usd_per_million([0.10, 0.01, 0.125, 0.50]).unwrap()
    }

    fn system() -> String {
        serde_json::from_str(SYSTEM_MESSAGE_JSON).unwrap()
    }

    fn reply(content: &str, finish: &str) -> Value {
        json!({"model":"gpt-5.6-luna","choices":[{"finish_reason":finish,"message":{"content":content}}],"usage":{"prompt_tokens":100,"completion_tokens":20}})
    }

    fn parse(v: &Value) -> Result<Answer, ReplyError> {
        parse_reply(v.to_string().as_bytes(), &luna56())
    }

    /// Ported verbatim on `parse_reply`.
    #[test]
    fn rejects_invented_sources_and_truncation() {
        let mut v = reply(
            "{\"answer\":\"hello\",\"sources\":[\"pinecone-fake\"]}",
            "stop",
        );
        assert!(parse(&v).is_err());
        v["choices"][0]["message"]["content"] =
            json!("{\"answer\":\"hello\",\"sources\":[\"projects\"]}");
        assert!(parse(&v).is_ok());
        v["choices"][0]["finish_reason"] = json!("length");
        assert!(parse(&v).is_err());
    }

    /// Ported: history at `messages[2]`, `reasoning_effort:"none"`; the reservation equals
    /// `reserve_nusd(body.len())`, which is at least the exact formula; fast body == reference.
    #[test]
    fn bounded_history_reaches_provider_and_reservation_is_conservative() {
        let history = vec![
            Message {
                role: "user".into(),
                content: "Sony?".into(),
            },
            Message {
                role: "assistant".into(),
                content: "Games.".into(),
            },
        ];
        let body = build_body("gpt-5.6-luna", "2026-11-24", &history, "And which tools?");
        let b: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(b["messages"][2]["content"], "Sony?");
        assert_eq!(b["reasoning_effort"], "none");
        let rates = luna56();
        let reservation = reserve_nusd(body.len(), &rates);
        let exact_pico = (body.len() as i128 + 512) * 250_000 + 640 * 1_200_000;
        assert!(i128::from(reservation) * 1000 >= exact_pico);
        assert!(i128::from(reservation) * 1000 - exact_pico < 1000);
        assert_eq!(
            body,
            build_body_reference(
                &system(),
                "gpt-5.6-luna",
                "2026-11-24",
                &history,
                "And which tools?"
            )
        );
    }

    #[test]
    fn fast_body_equals_reference_body() {
        let msg = |role: &str, content: &str| Message {
            role: role.into(),
            content: content.into(),
        };
        let histories: Vec<Vec<Message>> = vec![
            vec![],
            vec![
                msg("user", "Sony? \"quoted\" \\ back"),
                msg("assistant", "Games."),
            ],
            vec![
                msg("user", "é😀 non-ASCII \u{2028} line sep"),
                msg("assistant", "controls \u{0}\u{1f}\u{7f} tab\t nl\n cr\r"),
                msg("user", "  untrimmed  "),
                msg("assistant", "</script> & <b>"),
                msg("user", "third"),
                msg("assistant", "x".repeat(5000).as_str()),
            ],
        ];
        for history in &histories {
            for question in ["Micro Mages?", "  spaced\t", "\"q\"\\é😀\u{1}"] {
                for model in ["gpt-6-luna", "gpt-5.6-luna", "a\"b"] {
                    assert_eq!(
                        build_body(model, "2026-11-24", history, question),
                        build_body_reference(&system(), model, "2026-11-24", history, question)
                    );
                }
            }
        }
    }

    /// Golden sizes and the reservation of the "Micro Mages?" body (critique appendix B).
    #[test]
    fn reservation_vectors() {
        let b6 = build_body("gpt-6-luna", "2026-11-24", &[], "Micro Mages?");
        let b56 = build_body("gpt-5.6-luna", "2026-11-24", &[], "Micro Mages?");
        assert_eq!(b56.len(), b6.len() + 2);
        // Everything but the system message: framing, date line, question and model.
        assert_eq!(b6.len() - SYSTEM_MESSAGE_JSON.len(), 307);
        if system().len() == 16_078 {
            // The knowledge of release 1.0.39, cross-checked against the original server.
            assert_eq!(b6.len(), 17_184);
            assert_eq!(b56.len(), 17_186);
            assert_eq!(reserve_nusd(b6.len(), &luna6()), 2_532_000);
            assert_eq!(reserve_nusd(b56.len(), &luna56()), 5_192_500);
        }
        // Worst case: 4-byte characters filling the 64 KiB request cap (limits count characters).
        let four = "😀";
        let request = |assistant_chars: usize| {
            let history: Vec<Message> = (0..6)
                .map(|i| Message {
                    role: if i % 2 == 0 { "user" } else { "assistant" }.into(),
                    content: four.repeat(if i % 2 == 0 { 1200 } else { assistant_chars }),
                })
                .collect();
            let question = four.repeat(1200);
            let json = json!({"question": question, "history": history}).to_string();
            (json.len(), history, question)
        };
        let fitting = (0..=5000).rev().find(|n| request(*n).0 <= 65_536).unwrap();
        let (request_len, history, question) = request(fitting);
        assert!(request_len > 65_400);
        let worst = build_body("gpt-6-luna", "2026-11-24", &history, &question);
        assert!(
            worst.len() > 81_000 && worst.len() < 83_000,
            "{}",
            worst.len()
        );
        let reservation = reserve_nusd(worst.len(), &luna6());
        assert!(
            reservation > 10_400_000 && reservation < 10_800_000,
            "{reservation}"
        );
    }

    #[test]
    fn every_parse_error_with_its_class() {
        let ok = "{\"answer\":\" hello \",\"sources\":[\"projects\",\"faq\",\"projects\"]}";
        let a = parse(&reply(ok, "stop")).unwrap();
        assert_eq!(a.text, "hello");
        assert_eq!(a.sources, ["projects", "faq"]);
        assert_eq!(a.model, "gpt-5.6-luna");
        assert_eq!(a.nusd, 44_000);
        assert_eq!(a.usage, json!({"prompt_tokens":100,"completion_tokens":20}));

        let err = |v: &Value| parse(v).unwrap_err();
        let not_json = parse_reply(b"<html>", &luna56()).unwrap_err();
        assert_eq!(
            (not_json.class, not_json.message.as_str(), not_json.nusd),
            ("invalid_json", "error decoding response body", None)
        );
        let e = err(&reply(ok, "length"));
        assert_eq!(
            (e.class, e.message.as_str(), e.nusd),
            ("incomplete", "Incomplete provider answer", Some(44_000))
        );
        let mut v = reply(ok, "stop");
        v["choices"][0]["message"]["content"] = json!(null);
        assert_eq!(err(&v).class, "missing_answer");
        let e = err(&reply("not json", "stop"));
        assert_eq!(
            (e.class, e.message.as_str()),
            ("invalid_json", "expected ident at line 1 column 2")
        );
        assert_eq!(
            err(&reply("{\"answer\":\"  \",\"sources\":[]}", "stop")).class,
            "invalid_answer"
        );
        let long = json!({"answer":"x".repeat(5001),"sources":[]}).to_string();
        assert_eq!(err(&reply(&long, "stop")).class, "invalid_answer");
        let at_limit = json!({"answer":"é".repeat(5000),"sources":[]}).to_string();
        assert!(parse(&reply(&at_limit, "stop")).is_ok());
        assert_eq!(
            err(&reply("{\"answer\":\"a\"}", "stop")).class,
            "missing_sources"
        );
        assert_eq!(
            err(&reply("{\"answer\":\"a\",\"sources\":[1]}", "stop")).class,
            "invalid_source"
        );
        let e = err(&reply(
            "{\"answer\":\"a\",\"sources\":[\"pinecone-fake\"]}",
            "stop",
        ));
        assert_eq!(
            (e.class, e.message.as_str(), e.nusd),
            ("unknown_source", "Unknown source ID", Some(44_000))
        );
        let mut v = reply(ok, "stop");
        v["usage"] = json!({"completion_tokens":20});
        let e = err(&v);
        assert_eq!(
            (e.class, e.message.as_str(), e.nusd),
            ("missing_usage", "Missing input usage", None)
        );
        assert_eq!(e.usage, Some(json!({"completion_tokens":20})));
        v["usage"] = json!({"prompt_tokens":1});
        assert_eq!(err(&v).message, "Missing output usage");
        v["usage"] = json!({"prompt_tokens":1,"completion_tokens":1,"prompt_tokens_details":{"cached_tokens":2}});
        assert_eq!(err(&v).class, "invalid_cache_usage");
        let mut v = reply(ok, "stop");
        v.as_object_mut().unwrap().remove("model");
        let e = err(&v);
        assert_eq!(
            (e.class, e.message.as_str(), e.nusd),
            ("missing_model", "Missing model", Some(44_000))
        );
        // The first failing check wins: a bad source is reported before missing usage.
        let mut v = reply("{\"answer\":\"a\",\"sources\":[\"x\"]}", "stop");
        v["usage"] = Value::Null;
        let e = err(&v);
        assert_eq!((e.class, e.nusd, e.usage), ("unknown_source", None, None));
    }
}
