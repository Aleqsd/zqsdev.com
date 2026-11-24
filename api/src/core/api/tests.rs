//! Native tests of the Worker handlers: a fake platform with a virtual clock, a scripted provider
//! and a real `Book` on SQLite behind a JSON round trip.

use serde_json::{json, Value};

use super::{retry_finish, DB_HIGH_BYTES};
use crate::core::config::Config;
use crate::core::embedded::{COMMIT, DATA_JSON, KNOWLEDGE_UPDATED, VERSION};
use crate::core::http::{Body, OutBody};
use crate::core::ledger::protocol::{AdmitOp, LedgerOp, LedgerReply, StatsReply};
use crate::core::provider::build_body;
use crate::core::reply::FALLBACK_ANSWER;
use crate::core::testkit::{
    body_json, body_text, config, ledger_config, openai_reply, send, FakePlatform, LedgerStep,
    ProviderStep, Req, ADMIN_TOKEN, T0_MS,
};

const LUNA56: (&str, &str) = ("OPENAI_MODEL", "gpt-5.6-luna");

fn platform() -> FakePlatform {
    FakePlatform::new(ledger_config(&[]))
}

fn ask(question: &str) -> Req {
    Req::post_json(
        "/api/ai",
        &json!({ "question": question, "history": [] }).to_string(),
    )
}

fn log_cmd(body: &str) -> Req {
    Req::post_json("/api/log/command", body)
}

fn admin(path: &str, body: Value) -> Req {
    Req::post_json(path, &body.to_string())
}

fn with_token(mut body: Value) -> Value {
    body["token"] = json!(ADMIN_TOKEN);
    body
}

fn stats(p: &FakePlatform) -> StatsReply {
    match p.op(&LedgerOp::Stats) {
        LedgerReply::Stats(s) => *s,
        other => panic!("{other:?}"),
    }
}

fn rows(p: &FakePlatform, sql: &str) -> Vec<Vec<crate::core::ledger::store::SqlValue>> {
    p.host.store.borrow_mut().rows(sql)
}

fn count(p: &FakePlatform, sql: &str) -> i64 {
    p.host.store.borrow_mut().count(sql)
}

fn fallback_reason(out: &crate::core::http::Out) -> String {
    let body = body_json(out);
    assert_eq!(body["ai_enabled"], false);
    assert_eq!(body["answer"], FALLBACK_ANSWER);
    assert_eq!(body["model"], Value::Null);
    assert_eq!(body["sources"], json!([]));
    assert_eq!(body["knowledge_updated"], KNOWLEDGE_UPDATED);
    body["reason"].as_str().unwrap().to_string()
}

// ---------------------------------------------------------------- ported from main.rs

/// gpt-5.6-luna prices pinned, as the original fixture pinned its rates.
#[tokio::test]
async fn paid_call_once_and_usage_reconciled() {
    let p = platform();
    let cfg = config(&[LUNA56]);
    p.script_provider(ProviderStep::reply(
        200,
        openai_reply(
            "gpt-5.6-luna",
            "Micro Mages used Python.",
            &["projects"],
            json!({"prompt_tokens":100,"completion_tokens":20}),
        ),
        700,
    ));
    let out = send(&p, &cfg, &ask("Micro Mages?")).await;
    assert_eq!(out.status, 200);
    assert_eq!(out.content_type, Some("application/json"));
    assert!(out.headers.is_empty());
    let body = body_json(&out);
    assert_eq!(body["sources"][0], "projects");
    assert_eq!(body["ai_enabled"], true);
    assert_eq!(body["reason"], Value::Null);
    assert_eq!(body["model"], "gpt-5.6-luna");
    assert_eq!(p.provider_calls(), 1);
    assert_eq!(out.retry, None);
    assert_eq!(count(&p, "SELECT COUNT(*) FROM questions"), 1);
    assert_eq!(count(&p, "SELECT nusd FROM questions"), 44_000);
    assert_eq!(count(&p, "SELECT state FROM questions"), 1);
    assert_eq!(count(&p, "SELECT latency_ms FROM questions"), 700);
    // Exactly the body the original built, on the virtual clock's date.
    let sent = p.provider_bodies.borrow()[0].clone();
    assert_eq!(
        sent,
        build_body("gpt-5.6-luna", "2026-11-24", &[], "Micro Mages?")
    );
    assert!(stats(&p).inflight.is_empty());
    let logged = p.logged("🤖 AI response completed");
    assert_eq!(logged.len(), 1);
    assert_eq!(logged[0]["cost_usd"], json!(0.000044));
    assert_eq!(logged[0]["latency_ms"], json!(700));
}

#[tokio::test]
async fn spending_and_concurrency_block_before_provider() {
    let p = FakePlatform::new(ledger_config(&[
        ("AI_MINUTE_BUDGET_USD", "0"),
        ("AI_HOUR_BUDGET_USD", "0"),
        ("AI_DAY_BUDGET_USD", "0"),
        ("AI_MONTH_BUDGET_USD", "0"),
    ]));
    let cfg = config(&[]);
    let out = send(&p, &cfg, &ask("Micro Mages?")).await;
    assert_eq!(out.status, 429);
    assert_eq!(fallback_reason(&out), "spending_limit");
    assert_eq!(p.provider_calls(), 0);

    let p = platform();
    for ip in ["198.51.100.1", "198.51.100.2"] {
        let reply = p.op(&LedgerOp::Admit(AdmitOp {
            qid: ip.into(),
            ip: ip.into(),
            ip_key: ip.into(),
            nusd: 1_000,
            question: "q".into(),
            question_len: 1,
        }));
        assert!(matches!(reply, LedgerReply::Admitted { .. }));
    }
    let out = send(&p, &cfg, &ask("Micro Mages?")).await;
    assert_eq!(out.status, 429);
    assert_eq!(fallback_reason(&out), "busy");
    assert_eq!(p.provider_calls(), 0);
}

#[tokio::test]
async fn upstream_failure_and_timeout_restore_classic_mode() {
    for (status, delay, expected, http) in [
        (429, 0, "provider_unavailable", 503),
        (200, 300, "timeout", 504),
    ] {
        let p = platform();
        let mut cfg = config(&[]);
        cfg.timing.deadline_ms = 80;
        cfg.timing.margin_ms = 10;
        p.script_provider(ProviderStep::reply(
            status,
            openai_reply(
                "gpt-6-luna",
                "x",
                &[],
                json!({"prompt_tokens":1,"completion_tokens":1}),
            ),
            delay,
        ));
        let out = send(&p, &cfg, &ask("Micro Mages?")).await;
        assert_eq!(out.status, http);
        assert_eq!(fallback_reason(&out), expected);
        assert_eq!(p.provider_calls(), 1);
        assert!(stats(&p).inflight.is_empty());
        let data: Value = serde_json::from_str(DATA_JSON).unwrap();
        assert!(data["experiences"][0]["company"]
            .as_str()
            .unwrap()
            .contains("Studi"));
    }
}

// ---------------------------------------------------------------- /api/ai

#[tokio::test]
async fn reasons_follow_the_ledger_and_configuration() {
    // AI not configured: blank key, invalid price, price of 1e12, disallowed base URL.
    for extra in [
        vec![("OPENAI_API_KEY", "   ")],
        vec![("AI_INPUT_USD_PER_MILLION", "abc")],
        vec![("AI_OUTPUT_USD_PER_MILLION", "1e12")],
        vec![("OPENAI_BASE_URL", "https://attacker.example/v1")],
    ] {
        let p = platform();
        let cfg = config(&extra);
        let out = send(&p, &cfg, &ask("hi")).await;
        assert_eq!(out.status, 503, "{extra:?}");
        assert_eq!(fallback_reason(&out), "ai_not_configured");
        assert!(p.ledger_calls.borrow().is_empty());
        assert_eq!(p.provider_calls(), 0);
    }
    // The Durable Object is unreachable, times out, or answers an error.
    for step in [
        LedgerStep::Fail("stub threw".into()),
        LedgerStep::Normal { latency_ms: 2_001 },
        LedgerStep::Reply(LedgerReply::error("storage")),
        LedgerStep::Reply(LedgerReply::Unknown),
        LedgerStep::Reply(LedgerReply::denied("future_reason", "x")),
    ] {
        let p = platform();
        p.script_ledger(step.clone());
        let out = send(&p, &config(&[]), &ask("hi")).await;
        assert_eq!(out.status, 503, "{step:?}");
        assert_eq!(fallback_reason(&out), "budget_unavailable");
        assert_eq!(p.provider_calls(), 0);
        assert_eq!(p.logged("🧾 Ledger unavailable").len(), 1);
    }
    // A failed admission commit denies the call as a failed ledger write did.
    let p = platform();
    p.host.store.borrow_mut().fail_next_commit = true;
    let out = send(&p, &config(&[]), &ask("hi")).await;
    assert_eq!(out.status, 429);
    assert_eq!(fallback_reason(&out), "spending_limit");
    assert_eq!(
        p.logged("⚠️ AI admission denied")[0]["detail"],
        "ledger_write_failed"
    );
    assert_eq!(count(&p, "SELECT COUNT(*) FROM questions"), 0);
    // Per-IP limit: the fifth question in one second.
    let p = platform();
    let cfg = config(&[]);
    for _ in 0..4 {
        assert_eq!(send(&p, &cfg, &ask("hi")).await.status, 200);
    }
    let out = send(&p, &cfg, &ask("hi")).await;
    assert_eq!(fallback_reason(&out), "request_limit");
    p.advance(1_000);
    assert_eq!(send(&p, &cfg, &ask("hi")).await.status, 200);
}

#[tokio::test]
async fn invalid_requests_get_the_400_fallback() {
    let p = platform();
    let cfg = config(&[]);
    let mut cases = vec![
        Req::post_json("/api/ai", r#"{"question":"hi"}"#).content_type(None),
        Req::post_json("/api/ai", r#"{"question":"hi"}"#).content_type(Some("text/plain")),
        Req::post_json("/api/ai", r#"{"question":"hi"}"#)
            .content_type(Some("application/json; charset")),
        Req::post_json("/api/ai", "{"),
        Req::post_json("/api/ai", r#"{"question":"hi","history":null}"#),
        Req::post_json("/api/ai", r#"{"question":"hi","extra":1}"#),
        Req::post_json("/api/ai", r#"{"question":"   "}"#),
        Req::post_json(
            "/api/ai",
            &json!({"question": "é".repeat(1201)}).to_string(),
        ),
        Req::post_json(
            "/api/ai",
            r#"{"question":"hi","history":[{"role":"system","content":"x"},{"role":"assistant","content":"y"}]}"#,
        ),
    ];
    let mut big = Req::post_json("/api/ai", r#"{"question":"hi"}"#);
    big.body = Body::TooLarge;
    cases.push(big);
    let mut declared = Req::post_json("/api/ai", r#"{"question":"hi"}"#);
    declared.content_length = Some("65537".into());
    cases.push(declared);
    for req in &cases {
        let out = send(&p, &cfg, req).await;
        assert_eq!(out.status, 400, "{:?}", req.body);
        assert_eq!(fallback_reason(&out), "invalid_request");
    }
    assert!(p.ledger_calls.borrow().is_empty());
    // Trailing bytes after the JSON value are accepted, as axum 0.7.9 does.
    let out = send(
        &p,
        &cfg,
        &Req::post_json("/api/ai", r#"{"question":"hi"}xyz"#),
    )
    .await;
    assert_eq!(out.status, 200);
    // `application/ld+json` and `Application/JSON; charset=utf-8` are JSON for axum.
    for ct in ["application/ld+json", "Application/JSON; charset=utf-8"] {
        p.advance(1_000);
        let out = send(&p, &cfg, &ask("hi").content_type(Some(ct))).await;
        assert_eq!(out.status, 200, "{ct}");
    }
}

#[tokio::test]
async fn timer_classification_and_worst_case_time() {
    // Slow admission (1 s) + slow provider: the deadline-derived timer fires → 504 timeout.
    let p = platform();
    let cfg = config(&[]);
    p.script_ledger(LedgerStep::Normal { latency_ms: 1_000 });
    p.script_provider(ProviderStep::reply(200, json!({}), 20_000));
    let out = send(&p, &cfg, &ask("hi")).await;
    assert_eq!(
        (out.status, fallback_reason(&out).as_str()),
        (504, "timeout")
    );
    assert_eq!(p.provider_timers.borrow()[0].as_millis(), 14_500);
    assert_eq!(p.clock_ms.get() - T0_MS, 15_500);
    assert_eq!(p.logged("⏱️ AI timeout; reservation retained").len(), 1);
    assert_eq!(
        rows(&p, "SELECT state, error FROM questions")[0][1].as_str(),
        Some("deadline")
    );

    // Fast admission + provider over 15 s: the 15 s cap fires → 503 provider_unavailable.
    let p = platform();
    p.script_provider(ProviderStep::reply(200, json!({}), 16_000));
    let out = send(&p, &cfg, &ask("hi")).await;
    assert_eq!(
        (out.status, fallback_reason(&out).as_str()),
        (503, "provider_unavailable")
    );
    assert_eq!(p.provider_timers.borrow()[0].as_millis(), 15_000);
    assert_eq!(
        rows(&p, "SELECT error FROM questions")[0][0].as_str(),
        Some("timeout")
    );
    // The reservation is kept.
    let reserved = count(&p, "SELECT nusd FROM questions");
    assert!(reserved > 2_500_000);

    // Worst case: admission just under 2 s, provider never answers, settlement 1.5 s.
    let p = platform();
    p.script_ledger(LedgerStep::Normal { latency_ms: 1_999 });
    p.script_ledger(LedgerStep::Normal { latency_ms: 1_500 });
    p.script_provider(ProviderStep::reply(200, json!({}), 60_000));
    let out = send(&p, &cfg, &ask("hi")).await;
    assert_eq!(out.status, 504);
    assert!(p.clock_ms.get() - T0_MS <= 17_500);
    assert_eq!(out.retry, None);
    // Same with a settlement that times out: still ≤ 17.5 s, and a retry is scheduled.
    let p = platform();
    p.script_ledger(LedgerStep::Normal { latency_ms: 1_999 });
    p.script_ledger(LedgerStep::Normal { latency_ms: 9_000 });
    p.script_provider(ProviderStep::reply(200, json!({}), 60_000));
    let out = send(&p, &cfg, &ask("hi")).await;
    assert_eq!(out.status, 504);
    assert_eq!(p.clock_ms.get() - T0_MS, 1_999 + 13_501 + 1_500);
    assert!(out.retry.is_some());
    // A deadline already spent by the admission: no provider call at all.
    let p = platform();
    let mut short = config(&[]);
    short.timing.deadline_ms = 600;
    p.script_ledger(LedgerStep::Normal { latency_ms: 200 });
    let out = send(&p, &short, &ask("hi")).await;
    assert_eq!((out.status, p.provider_calls()), (504, 0));
}

#[tokio::test]
async fn provider_failures_are_classified_and_settled() {
    let cfg = config(&[LUNA56]);
    let usage = json!({"prompt_tokens":100,"completion_tokens":20});
    // A 2xx reply with an unknown source: 503, charged its actual usage cost.
    let p = platform();
    p.script_provider(ProviderStep::reply(
        200,
        openai_reply("gpt-5.6-luna", "x", &["pinecone-fake"], usage.clone()),
        10,
    ));
    let out = send(&p, &cfg, &ask("hi")).await;
    assert_eq!(
        (out.status, fallback_reason(&out).as_str()),
        (503, "provider_unavailable")
    );
    let row = rows(&p, "SELECT state, nusd, error, usage FROM questions");
    assert_eq!(row[0][0].as_i64(), Some(2));
    assert_eq!(row[0][1].as_i64(), Some(44_000));
    assert_eq!(row[0][2].as_str(), Some("unknown_source"));
    assert_eq!(
        row[0][3].as_str(),
        Some(r#"{"completion_tokens":20,"prompt_tokens":100}"#)
    );
    let charged = p.logged("⚠️ AI failed; actual usage charged");
    assert_eq!(charged[0]["error"], "Unknown source ID");
    assert_eq!(charged[0]["cost_usd"], json!(0.000044));

    // Non-2xx, transport, oversize, non-JSON: reservation kept, class stored.
    for (step, class, message) in [
        (
            ProviderStep::reply(429, json!({}), 0),
            "http_429",
            "Provider HTTP 429",
        ),
        (
            ProviderStep::transport("connection reset"),
            "transport",
            "connection reset",
        ),
        (
            ProviderStep {
                status: 200,
                body: vec![b' '; 70_000],
                latency_ms: 0,
                transport_error: None,
            },
            "too_large",
            "Provider reply exceeds 64 KiB",
        ),
        (
            ProviderStep {
                status: 200,
                body: b"<html>".to_vec(),
                latency_ms: 0,
                transport_error: None,
            },
            "invalid_json",
            "error decoding response body",
        ),
    ] {
        let p = platform();
        p.script_provider(step);
        let out = send(&p, &cfg, &ask("hi")).await;
        assert_eq!(out.status, 503, "{class}");
        let row = rows(&p, "SELECT state, nusd, error FROM questions");
        assert_eq!(row[0][0].as_i64(), Some(2));
        assert!(
            row[0][1].as_i64().unwrap() > 5_000_000,
            "{class}: reservation kept"
        );
        assert_eq!(row[0][2].as_str(), Some(class));
        let logged = p.logged("⚠️ AI failed; reservation retained for uncertain usage");
        assert_eq!(logged[0]["error"], message);
    }
}

#[tokio::test]
async fn failed_settlement_is_retried_once() {
    let p = platform();
    let cfg = config(&[]);
    p.script_ledger(LedgerStep::Normal { latency_ms: 0 });
    p.script_ledger(LedgerStep::Fail("stub threw".into()));
    let out = send(&p, &cfg, &ask("Micro Mages?")).await;
    // The answer is sent anyway; the row is still in flight with its reservation.
    assert_eq!(out.status, 200);
    assert_eq!(count(&p, "SELECT state FROM questions"), 0);
    let qid = p.logged("🤖 AI response completed")[0]["question_id"].clone();
    assert!(qid.is_string());
    let settlement = p.logged("🧾 Settlement failed; reservation kept");
    assert_eq!(
        settlement[0],
        json!({"question_id": qid, "seq": 1, "error": "transport: stub threw", "retry": false})
    );
    let retry = out.retry.clone().unwrap();
    assert_eq!((json!(retry.question_id), retry.seq), (qid.clone(), 1));
    assert!(matches!(&retry.op, LedgerOp::Finish(f) if f.outcome == "answered" && f.seq == 1));
    // The glue runs it in wait_until after a second; a retry that lands is logged too.
    p.advance(cfg.timing.retry_delay_ms);
    assert!(retry_finish(&p, &cfg, &retry).await);
    assert_eq!(count(&p, "SELECT state FROM questions"), 1);
    assert_eq!(count(&p, "SELECT nusd FROM questions"), 20_000);
    assert!(stats(&p).inflight.is_empty());
    assert_eq!(
        p.logged("🧾 Settlement completed on retry"),
        [json!({"question_id": qid, "seq": 1})]
    );
    // Running it again is harmless (idempotent finish).
    assert!(retry_finish(&p, &cfg, &retry).await);
    // A retry that fails too is logged with the same question id; the lease frees the slot later.
    p.script_ledger(LedgerStep::Fail("again".into()));
    assert!(!retry_finish(&p, &cfg, &retry).await);
    assert_eq!(
        p.logged("🧾 Settlement failed; reservation kept")[1],
        json!({"question_id": qid, "seq": 1, "error": "transport: again", "retry": true})
    );
}

#[tokio::test]
async fn logged_rows_and_lines_carry_no_visitor_text() {
    let p = platform();
    let cfg = config(&[]);
    let question = "  My key is sk-abcdefghijklmnopqrstuvwxyz,\nok?  ";
    let req = Req::post_json(
        "/api/ai",
        &json!({"question": question, "history": [{"role":"user","content":"Sony?"},{"role":"assistant","content":"Games."}]}).to_string(),
    )
    .ip(Some("2001:db8:1:2:3:4:5:6"));
    let out = send(&p, &cfg, &req).await;
    assert_eq!(out.status, 200);
    let row = rows(
        &p,
        "SELECT question, question_len, ip, answer, answer_len, answer_id FROM questions",
    );
    assert_eq!(
        row[0][0].as_str(),
        Some("My key is [redacted-openai-key], ok?")
    );
    assert_eq!(row[0][1].as_i64(), Some(question.chars().count() as i64));
    assert_eq!(row[0][2].as_str(), Some("2001:db8:1:2:3:4:5:6"));
    assert_eq!(row[0][3].as_str(), Some("Micro Mages used Python."));
    assert_eq!(row[0][4].as_i64(), Some(24));
    assert!(row[0][5].as_str().is_some());
    assert_eq!(
        count(
            &p,
            "SELECT COUNT(*) FROM ip_windows WHERE ip_key = '2001:db8:1:2::/64'"
        ),
        1
    );
    let sent = p.provider_bodies.borrow()[0].clone();
    assert!(sent.contains(r#"{"content":"Sony?","role":"user"}"#));
    assert!(sent.contains(&serde_json::to_string(question.trim()).unwrap()));
    for (_, msg, fields) in p.logs.borrow().iter() {
        let text = format!("{msg} {fields}");
        assert!(
            !text.contains("sk-abc") && !text.contains("My key"),
            "{text}"
        );
        assert!(!text.contains("2001:db8"), "{text}");
        assert!(!text.contains("Python"), "{text}");
    }
}

// ---------------------------------------------------------------- /api/log/command

#[tokio::test]
async fn command_log_rules() {
    let p = platform();
    let cfg = config(&[]);
    let status = |out: crate::core::http::Out| (out.status, out.body == OutBody::Empty);
    assert_eq!(
        status(
            send(
                &p,
                &cfg,
                &log_cmd(r#"{"command":"projects","mode":"classic"}"#)
            )
            .await
        ),
        (204, true)
    );
    for bad in [r#"{"command":""}"#, r#"{"command":"  \t "}"#] {
        assert_eq!(
            status(send(&p, &cfg, &log_cmd(bad)).await),
            (400, true),
            "{bad}"
        );
    }
    let ok = json!({"command": "é".repeat(2000)}).to_string();
    assert_eq!(send(&p, &cfg, &log_cmd(&ok)).await.status, 204);
    let long = json!({"command": "é".repeat(2001)}).to_string();
    assert_eq!(send(&p, &cfg, &log_cmd(&long)).await.status, 400);
    for (body, stored) in [
        (r#"{"command":"a","mode":null}"#, "classic"),
        (r#"{"command":"b"}"#, "classic"),
        (r#"{"command":"c","mode":"  "}"#, "classic"),
        (r#"{"command":"d","mode":" ai "}"#, "ai"),
        (r#"{"command":"e","unknown":[1,2],"mode":"ai"}"#, "ai"),
        (r#"{"command":"f"} garbage"#, "classic"),
    ] {
        assert_eq!(send(&p, &cfg, &log_cmd(body)).await.status, 204, "{body}");
        let mode = rows(&p, "SELECT mode FROM commands ORDER BY seq DESC LIMIT 1");
        assert_eq!(mode[0][0].as_str(), Some(stored), "{body}");
    }
    assert_eq!(
        send(&p, &cfg, &log_cmd(r#"{"command":"x","mode":"AI"}"#))
            .await
            .status,
        400
    );
    // 250 characters are stored as 200 plus the truncation suffix; the length is kept.
    let command = "y".repeat(250);
    let out = send(
        &p,
        &cfg,
        &log_cmd(&json!({ "command": command }).to_string()),
    )
    .await;
    assert_eq!(out.status, 204);
    let row = rows(
        &p,
        "SELECT command, command_len, ip FROM commands ORDER BY seq DESC LIMIT 1",
    );
    assert_eq!(
        row[0][0].as_str().map(str::to_string),
        Some(format!("{} [truncated 50 chars]", "y".repeat(200)))
    );
    assert_eq!(row[0][1].as_i64(), Some(250));
    assert_eq!(row[0][2].as_str(), Some("203.0.113.7"));
    // Trimmed command, sanitized.
    send(
        &p,
        &cfg,
        &log_cmd(r#"{"command":"  cat\u0000 key sk-abcdefghijklmnopqrstuvwxyz  "}"#),
    )
    .await;
    let row = rows(
        &p,
        "SELECT command, command_len FROM commands ORDER BY seq DESC LIMIT 1",
    );
    assert_eq!(row[0][0].as_str(), Some("cat key [redacted-openai-key]"));
    assert_eq!(row[0][1].as_i64(), Some(38));
}

#[tokio::test]
async fn command_log_rejections_match_axum() {
    let p = platform();
    let cfg = config(&[]);
    let check = |out: crate::core::http::Out, status: u16, text: &str| {
        assert_eq!(out.status, status);
        assert_eq!(out.content_type, Some("text/plain; charset=utf-8"));
        assert_eq!(body_text(&out), text);
    };
    check(
        send(&p, &cfg, &log_cmd(r#"{"command":"x"}"#).content_type(None)).await,
        415,
        "Expected request with `Content-Type: application/json`",
    );
    check(
        send(
            &p,
            &cfg,
            &log_cmd(r#"{"command":"x"}"#).content_type(Some("application/json; =x")),
        )
        .await,
        415,
        "Expected request with `Content-Type: application/json`",
    );
    let at_cap = format!(r#"{{"command":"{}"}}"#, "x".repeat(65_536 - 14));
    assert_eq!(at_cap.len(), 65_536);
    // 65,536 bytes are read; the command is then too long (400, empty body).
    let out = send(&p, &cfg, &log_cmd(&at_cap)).await;
    assert_eq!((out.status, out.body.clone()), (400, OutBody::Empty));
    let over = format!(r#"{{"command":"{}"}}"#, "x".repeat(65_536 - 13));
    check(
        send(&p, &cfg, &log_cmd(&over)).await,
        413,
        "Failed to buffer the request body: length limit exceeded",
    );
    check(
        send(&p, &cfg, &log_cmd(r#"{"mode":"ai"}"#)).await,
        422,
        "Failed to deserialize the JSON body into the target type: missing field `command` at line 1 column 13",
    );
    check(
        send(&p, &cfg, &log_cmd(r#"{"command":["x"]}"#)).await,
        422,
        "Failed to deserialize the JSON body into the target type: command: invalid type: sequence, expected a string at line 1 column 11",
    );
    check(
        send(&p, &cfg, &log_cmd("nope")).await,
        400,
        "Failed to parse the request body as JSON: expected ident at line 1 column 2",
    );
    let mut unreadable = log_cmd("");
    unreadable.body = Body::Unreadable("Network connection lost.".into());
    check(
        send(&p, &cfg, &unreadable).await,
        400,
        "Failed to buffer the request body: Network connection lost.",
    );
    assert!(p.ledger_calls.borrow().is_empty());
}

#[tokio::test]
async fn command_log_limits_and_failures() {
    // Window: 120 lines per 60 s, global; the mode check comes first.
    let p = platform();
    let cfg = config(&[]);
    for _ in 0..120 {
        assert_eq!(
            send(&p, &cfg, &log_cmd(r#"{"command":"ls"}"#)).await.status,
            204
        );
    }
    let full = send(
        &p,
        &cfg,
        &log_cmd(r#"{"command":"ls"}"#).ip(Some("198.51.100.9")),
    )
    .await;
    assert_eq!((full.status, full.body.clone()), (429, OutBody::Empty));
    assert_eq!(
        send(&p, &cfg, &log_cmd(r#"{"command":"ls","mode":"bad"}"#))
            .await
            .status,
        400
    );
    p.advance(60_000);
    assert_eq!(
        send(&p, &cfg, &log_cmd(r#"{"command":"ls"}"#)).await.status,
        204
    );

    // Daily cap.
    let p = FakePlatform::new(ledger_config(&[("COMMAND_LOG_DAILY_CAP", "120")]));
    for _ in 0..120 {
        assert_eq!(
            send(&p, &cfg, &log_cmd(r#"{"command":"ls"}"#)).await.status,
            204
        );
        p.advance(1_000);
    }
    assert_eq!(
        send(&p, &cfg, &log_cmd(r#"{"command":"ls"}"#)).await.status,
        429
    );
    p.advance(86_400_000);
    assert_eq!(
        send(&p, &cfg, &log_cmd(r#"{"command":"ls"}"#)).await.status,
        204
    );

    // Ledger failures: 500, logged.
    let p = platform();
    for step in [
        LedgerStep::Fail("stub threw".into()),
        LedgerStep::Normal { latency_ms: 2_500 },
        LedgerStep::Reply(LedgerReply::error("storage")),
    ] {
        p.script_ledger(step);
        let out = send(&p, &cfg, &log_cmd(r#"{"command":"ls"}"#)).await;
        assert_eq!((out.status, out.body.clone()), (500, OutBody::Empty));
    }
    assert_eq!(p.logged("📝 Failed to persist command log entry").len(), 3);

    // Missing CF-Connecting-IP: stored as unknown, warned once per minute.
    let p = platform();
    for _ in 0..3 {
        send(&p, &cfg, &log_cmd(r#"{"command":"ls"}"#).ip(None)).await;
    }
    assert_eq!(
        count(&p, "SELECT COUNT(*) FROM commands WHERE ip = 'unknown'"),
        3
    );
    let warned = p.logged("🌐 Missing CF-Connecting-IP");
    assert_eq!(warned.len(), 1);
    assert_eq!(warned[0]["route"], "/api/log/command");
    p.advance(60_000);
    send(&p, &cfg, &log_cmd(r#"{"command":"ls"}"#).ip(Some("bad"))).await;
    let warned = p.logged("🌐 Missing CF-Connecting-IP");
    assert_eq!(warned[1]["suppressed"], 2);
}

// ---------------------------------------------------------------- data, version, health, routing

#[tokio::test]
async fn data_version_and_routes() {
    let p = platform();
    let cfg = config(&[]);
    let data = send(&p, &cfg, &Req::get("/api/data")).await;
    assert_eq!(data.status, 200);
    assert_eq!(data.body.as_bytes(), DATA_JSON.as_bytes());
    assert_eq!(data.content_type, Some("application/json"));
    assert_eq!(
        data.header_value("Cache-Control"),
        Some("public, max-age=60, must-revalidate")
    );
    let head = send(&p, &cfg, &Req::get("/api/data").method("HEAD")).await;
    assert_eq!(head.body, OutBody::Empty);
    assert_eq!(head.headers, data.headers);
    assert_eq!(head.content_type, data.content_type);

    let version = send(&p, &cfg, &Req::get("/api/version")).await;
    assert_eq!(version.status, 200);
    assert_eq!(
        body_text(&version),
        format!(
            r#"{{"ai_configured":true,"commit":"{COMMIT}","knowledge_mode":"full_context","knowledge_updated":"{KNOWLEDGE_UPDATED}","model":"gpt-6-luna","version":"{VERSION}"}}"#
        )
    );
    assert!(version.headers.is_empty());
    let off = Config::from_lookup(&|_: &str| None);
    let version = send(&p, &off, &Req::get("/api/version")).await;
    assert!(body_text(&version).starts_with(r#"{"ai_configured":false,"#));

    for (method, path, allow) in [
        ("POST", "/api/data", "GET,HEAD"),
        ("PUT", "/api/version", "GET,HEAD"),
        ("OPTIONS", "/api/health", "GET,HEAD"),
        ("GET", "/api/ai", "POST"),
        ("HEAD", "/api/ai", "POST"),
        ("OPTIONS", "/api/ai", "POST"),
        ("GET", "/api/log/command", "POST"),
    ] {
        let out = send(&p, &cfg, &Req::get(path).method(method)).await;
        assert_eq!(
            (out.status, out.header_value("Allow")),
            (405, Some(allow)),
            "{method} {path}"
        );
        assert_eq!(out.body, OutBody::Empty);
    }
    for path in ["/api/data/", "/api/unknown", "/api", "/api/ai/"] {
        for method in ["GET", "POST", "DELETE"] {
            let out = send(&p, &cfg, &Req::get(path).method(method)).await;
            assert_eq!(out.status, 404, "{method} {path}");
            assert_eq!(out.header_value("Cache-Control"), Some("no-store"));
            assert_eq!(out.body, OutBody::Empty);
        }
    }
    assert!(p.ledger_calls.borrow().is_empty());
}

#[tokio::test]
async fn health_reports_each_cause() {
    let cfg = config(&[]);
    let p = platform();
    let out = send(&p, &cfg, &Req::get("/api/health")).await;
    assert_eq!(out.status, 200);
    assert_eq!(
        body_text(&out),
        r#"{"ok":true,"ai_configured":true,"ledger":"ok","storage":"ok"}"#
    );
    assert_eq!(out.header_value("Cache-Control"), Some("no-store"));
    let head = send(&p, &cfg, &Req::get("/api/health").method("HEAD")).await;
    assert_eq!((head.status, head.body), (200, OutBody::Empty));

    let cases: [(Option<LedgerStep>, Config, &str); 6] = [
        (
            None,
            Config::from_lookup(&|_: &str| None),
            r#"{"ok":false,"ai_configured":false,"ledger":"ok","storage":"ok"}"#,
        ),
        (
            Some(LedgerStep::Fail("down".into())),
            config(&[]),
            r#"{"ok":false,"ai_configured":true,"ledger":"unavailable","storage":"unknown"}"#,
        ),
        (
            Some(LedgerStep::Normal { latency_ms: 2_001 }),
            config(&[]),
            r#"{"ok":false,"ai_configured":true,"ledger":"unavailable","storage":"unknown"}"#,
        ),
        (
            Some(LedgerStep::Reply(LedgerReply::Pong {
                ledger_ok: false,
                text_full: false,
                db_bytes: 1,
            })),
            config(&[]),
            r#"{"ok":false,"ai_configured":true,"ledger":"invalid","storage":"ok"}"#,
        ),
        (
            Some(LedgerStep::Reply(LedgerReply::Pong {
                ledger_ok: true,
                text_full: true,
                db_bytes: 1,
            })),
            config(&[]),
            r#"{"ok":false,"ai_configured":true,"ledger":"ok","storage":"text_full"}"#,
        ),
        (
            Some(LedgerStep::Reply(LedgerReply::Pong {
                ledger_ok: true,
                text_full: false,
                db_bytes: DB_HIGH_BYTES + 1,
            })),
            config(&[]),
            r#"{"ok":false,"ai_configured":true,"ledger":"ok","storage":"db_high"}"#,
        ),
    ];
    for (step, cfg, expected) in cases {
        let p = platform();
        if let Some(step) = step {
            p.script_ledger(step);
        }
        let out = send(&p, &cfg, &Req::get("/api/health")).await;
        assert_eq!((out.status, body_text(&out).as_str()), (503, expected));
    }
    // A real invalid ledger.
    let p = platform();
    send(&p, &cfg, &log_cmd(r#"{"command":"ls"}"#)).await;
    p.host
        .store
        .borrow()
        .conn
        .execute_batch("UPDATE meta SET sum_day = -1")
        .unwrap();
    p.host.evict();
    let out = send(&p, &cfg, &Req::get("/api/health")).await;
    assert_eq!(body_json(&out)["ledger"], "invalid");
}

// ---------------------------------------------------------------- /api/admin/*

#[tokio::test]
async fn admin_rejections_are_plain_404s() {
    let p = platform();
    let cfg = config(&[]);
    let cases = [
        admin("/api/admin/stats", json!({})),
        admin(
            "/api/admin/stats",
            json!({"token": "wrong-token-0123456789abcdefghijklmnop"}),
        ),
        admin("/api/admin/stats", json!({"token": ADMIN_TOKEN})).method("GET"),
        admin("/api/admin/unknown", with_token(json!({}))),
        admin("/api/admin/stats", with_token(json!({}))).content_type(Some("text/plain")),
        Req::post_json("/api/admin/stats", "not json"),
    ];
    for req in &cases {
        let out = send(&p, &cfg, req).await;
        assert_eq!(out.status, 404, "{} {:?}", req.path, req.body);
        assert_eq!(out.body, OutBody::Empty);
        assert_eq!(out.header_value("Cache-Control"), Some("no-store"));
    }
    // A 31-character secret disables admin entirely.
    let short = "s".repeat(31);
    let weak = Config::from_lookup(&crate::core::testkit::lookup(&[("ADMIN_TOKEN", &short)]));
    let out = send(
        &p,
        &weak,
        &admin("/api/admin/stats", json!({"token": short})),
    )
    .await;
    assert_eq!(out.status, 404);
    assert!(p.ledger_calls.borrow().is_empty());
    let rejected = p.logged("🔐 Admin request rejected");
    assert_eq!(rejected.len(), 1);
    assert!(!rejected[0].to_string().contains(ADMIN_TOKEN));
}

#[tokio::test]
async fn admin_operations() {
    let p = platform();
    let cfg = config(&[]);
    // A few rows to look at.
    for _ in 0..3 {
        assert_eq!(send(&p, &cfg, &ask("Micro Mages?")).await.status, 200);
        p.advance(2_000);
    }
    send(&p, &cfg, &log_cmd(r#"{"command":"projects"}"#)).await;

    let out = send(&p, &cfg, &admin("/api/admin/stats", with_token(json!({})))).await;
    assert_eq!(out.status, 200);
    assert_eq!(out.header_value("Cache-Control"), Some("no-store"));
    let s = body_json(&out);
    assert_eq!(s["status"], "stats");
    assert_eq!(s["questions_rows"], 3);
    assert_eq!(s["valid"], true);

    let out = send(
        &p,
        &cfg,
        &admin(
            "/api/admin/export",
            with_token(json!({"table":"questions","limit":2})),
        ),
    )
    .await;
    assert_eq!(out.status, 200);
    assert_eq!(out.content_type, Some("application/x-ndjson"));
    assert_eq!(out.header_value("X-Next-After"), Some("2"));
    assert_eq!(body_text(&out).lines().count(), 6);
    let out = send(
        &p,
        &cfg,
        &admin(
            "/api/admin/export",
            with_token(json!({"table":"questions","after":"2"})),
        ),
    )
    .await;
    assert_eq!(out.header_value("X-Next-After"), None);
    assert_eq!(body_text(&out).lines().count(), 3);
    for (body, code) in [
        (json!({"table":"meta"}), "invalid_table"),
        (json!({"table":"ledger","after":"5"}), "invalid_cursor"),
        (json!({"table":"commands","limit":"x"}), "invalid_limit"),
    ] {
        let out = send(&p, &cfg, &admin("/api/admin/export", with_token(body))).await;
        assert_eq!(
            (out.status, body_text(&out)),
            (400, format!(r#"{{"error":"{code}"}}"#))
        );
    }

    let charges = json!({"charges":[{"id":"vps-1","at":1_795_478_000,"usd":0.000873}]});
    let out = send(
        &p,
        &cfg,
        &admin("/api/admin/import-ledger", with_token(charges.clone())),
    )
    .await;
    assert_eq!(
        body_text(&out),
        r#"{"status":"imported","inserted":1,"updated":0,"skipped":0}"#
    );
    assert_eq!(p.logged("📥 Ledger import chunk")[0]["inserted"], 1);
    let bad = json!({"charges":[{"id":"","at":1,"usd":1}]});
    let out = send(
        &p,
        &cfg,
        &admin("/api/admin/import-ledger", with_token(bad)),
    )
    .await;
    assert_eq!(
        (out.status, body_text(&out).as_str()),
        (400, r#"{"error":"invalid_charge:0"}"#)
    );
    let many: Vec<Value> = (0..=crate::core::ledger::IMPORT_MAX)
        .map(|i| json!({"id":i.to_string(),"at":1,"usd":0}))
        .collect();
    let out = send(
        &p,
        &cfg,
        &admin(
            "/api/admin/import-ledger",
            with_token(json!({"charges": many})),
        ),
    )
    .await;
    assert_eq!(
        (out.status, body_text(&out).as_str()),
        (400, r#"{"error":"too_many_charges"}"#)
    );
    let out = send(
        &p,
        &cfg,
        &admin("/api/admin/import-ledger/close", with_token(json!({}))),
    )
    .await;
    assert_eq!(body_text(&out), r#"{"status":"done"}"#);
    let out = send(
        &p,
        &cfg,
        &admin("/api/admin/import-ledger", with_token(charges)),
    )
    .await;
    assert_eq!(
        (out.status, body_text(&out).as_str()),
        (400, r#"{"error":"import_closed"}"#)
    );

    let out = send(
        &p,
        &cfg,
        &admin(
            "/api/admin/purge",
            with_token(json!({"table":"commands","before":1})),
        ),
    )
    .await;
    assert_eq!(
        body_text(&out),
        r#"{"status":"purged","deleted":0,"more":false}"#
    );
    let out = send(
        &p,
        &cfg,
        &admin(
            "/api/admin/purge",
            with_token(json!({"table":"ledger","before":1})),
        ),
    )
    .await;
    assert_eq!(out.status, 400);
    let out = send(
        &p,
        &cfg,
        &admin("/api/admin/purge", with_token(json!({"table":"commands"}))),
    )
    .await;
    assert_eq!(
        (out.status, body_text(&out).as_str()),
        (400, r#"{"error":"invalid_before"}"#)
    );

    let out = send(
        &p,
        &cfg,
        &admin("/api/admin/rebuild", with_token(json!({}))),
    )
    .await;
    assert_eq!(
        (out.status, body_text(&out).as_str()),
        (200, r#"{"status":"done"}"#)
    );

    // An unreachable Durable Object.
    p.script_ledger(LedgerStep::Fail("down".into()));
    let out = send(&p, &cfg, &admin("/api/admin/stats", with_token(json!({})))).await;
    assert_eq!(
        (out.status, body_text(&out).as_str()),
        (503, r#"{"error":"ledger_unavailable"}"#)
    );
    p.script_ledger(LedgerStep::Normal { latency_ms: 5_001 });
    let out = send(
        &p,
        &cfg,
        &admin("/api/admin/export", with_token(json!({"table":"ledger"}))),
    )
    .await;
    assert_eq!(out.status, 503);
}
