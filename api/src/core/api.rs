//! The API Worker's request handling: `/api/data`, `/api/version`, `/api/health`, `/api/ai`,
//! `/api/log/command` and `/api/admin/*` (design section 2), independent of the runtime.

use std::time::Duration;

use serde_json::{json, Value};

use super::auth::token_matches;
use super::client_ip::{self, ClientIp};
use super::config::Config;
use super::embedded::DATA_JSON;
use super::http::{
    effective_body, extract_json, json_content_type, AdminRoute, Body, In, Out, OutBody, Route,
    SettlementRetry, DATA_CACHE_CONTROL, NDJSON, NO_STORE,
};
use super::ledger::export::{parse_cursor, Table};
use super::ledger::protocol::{
    AdmitOp, CommandOp, ExportOp, FinishOp, ImportOp, LedgerOp, LedgerReply, PurgeOp,
};
use super::ledger::IMPORT_MAX;
use super::logline::Level;
use super::money::{reserve_nusd, usd};
use super::platform::{log_throttled, Platform, PlatformError, ProviderError, ProviderHttp};
use super::provider::{build_body, parse_reply, Answer, ReplyError};
use super::reply::{error_json, health_json, success_json, Reason};
use super::request::{command_mode, valid_command, AiRequest, CommandLogRequest};
use super::sanitize::{sanitize_command, sanitize_log_text};
use super::time::utc_date;

/// `/api/health` turns `ok:false` above this database size.
pub const DB_HIGH_BYTES: i64 = 768 * 1_048_576;

/// Handles one request. Never fails: every error becomes a response.
pub async fn handle(p: &impl Platform, cfg: &Config, req: In<'_>) -> Out {
    let head = req.method == "HEAD";
    let out = match super::http::route(req.method, req.path) {
        Route::Data => {
            Out::json_static(200, DATA_JSON).header("Cache-Control", DATA_CACHE_CONTROL.to_string())
        }
        Route::Version => Out::json(200, cfg.version_body.clone()),
        Route::Health => health(p, cfg).await,
        Route::Ai => ai(p, cfg, req).await,
        Route::CommandLog => command_log(p, cfg, req).await,
        Route::Admin(op) => admin(p, cfg, op, req).await,
        Route::AdminRejected => admin_rejected(p, req.path),
        Route::NotAllowed(allow) => Out::method_not_allowed(allow),
        Route::NotFound => Out::not_found(),
    };
    if head {
        out.without_body()
    } else {
        out
    }
}

fn millis(ms: u64) -> Duration {
    Duration::from_millis(ms)
}

/// The visitor IP; a missing header is logged (throttled), never the address itself.
fn visitor(p: &impl Platform, header: Option<&str>, route: &'static str) -> ClientIp {
    let ip = client_ip::resolve(header);
    if !ip.known {
        log_throttled(
            p,
            Level::Warn,
            "🌐 Missing CF-Connecting-IP",
            json!({ "route": route }),
        );
    }
    ip
}

fn ledger_unavailable(p: &impl Platform, route: &str, error: &str) {
    p.log(
        Level::Warn,
        "🧾 Ledger unavailable",
        json!({ "route": route, "error": error }),
    );
}

// ---------------------------------------------------------------- /api/health

async fn health(p: &impl Platform, cfg: &Config) -> Out {
    let reply = p
        .ledger(&LedgerOp::Ping, millis(cfg.timing.ping_timeout_ms))
        .await;
    let (ledger, storage) = match reply {
        Ok(LedgerReply::Pong {
            ledger_ok,
            text_full,
            db_bytes,
        }) => (
            if ledger_ok { "ok" } else { "invalid" },
            if db_bytes > DB_HIGH_BYTES {
                "db_high"
            } else if text_full {
                "text_full"
            } else {
                "ok"
            },
        ),
        Ok(other) => {
            ledger_unavailable(
                p,
                "/api/health",
                &format!("unexpected reply: {}", other.status()),
            );
            ("unavailable", "unknown")
        }
        Err(error) => {
            ledger_unavailable(p, "/api/health", &error.to_string());
            ("unavailable", "unknown")
        }
    };
    let ok = cfg.ai_configured && ledger == "ok" && storage == "ok";
    Out::json(
        if ok { 200 } else { 503 },
        health_json(ok, cfg.ai_configured, ledger, storage),
    )
    .header("Cache-Control", NO_STORE.to_string())
}

// ---------------------------------------------------------------- /api/ai

fn fallback(reason: Reason) -> Out {
    Out::json_static(reason.status(), reason.fallback_body())
}

/// How one provider call ended.
enum Outcome {
    Answered(Answer),
    Failed {
        reason: Reason,
        class: String,
        message: String,
        nusd: Option<i64>,
        usage: Option<Value>,
    },
}

async fn ai(p: &impl Platform, cfg: &Config, req: In<'_>) -> Out {
    let t0 = p.now_ms();
    // Every extractor failure (content type, size, JSON) and every validation failure: 400.
    let Ok(request) = extract_json::<AiRequest>(req.content_type, req.content_length, req.body)
    else {
        return fallback(Reason::InvalidRequest);
    };
    if request.validate().is_err() {
        return fallback(Reason::InvalidRequest);
    }
    if !cfg.ai_configured {
        return fallback(Reason::AiNotConfigured);
    }
    let ip = visitor(p, req.cf_connecting_ip, "/api/ai");
    let qid = p.uuid();
    let now_s = i64::try_from(t0 / 1000).unwrap_or(i64::MAX);
    let body = build_body(
        &cfg.model,
        &utc_date(now_s),
        &request.history,
        &request.question,
    );
    let reservation = reserve_nusd(body.len(), &cfg.rates);
    let admit = LedgerOp::Admit(AdmitOp {
        qid: qid.clone(),
        ip: ip.ip,
        ip_key: ip.key,
        nusd: reservation,
        question: sanitize_log_text(&request.question),
        question_len: i64::try_from(request.question.chars().count()).unwrap_or(i64::MAX),
    });

    let seq = match p.ledger(&admit, millis(cfg.timing.admit_timeout_ms)).await {
        Ok(LedgerReply::Admitted { seq }) => seq,
        Ok(LedgerReply::Denied { reason, detail }) => {
            let detail = detail.unwrap_or_default();
            return match Reason::from_denial(&reason) {
                Some(Reason::SpendingLimit) => {
                    log_throttled(
                        p,
                        Level::Warn,
                        "⚠️ AI admission denied",
                        json!({ "detail": detail }),
                    );
                    fallback(Reason::SpendingLimit)
                }
                Some(Reason::BudgetUnavailable) => {
                    ledger_unavailable(p, "/api/ai", &detail);
                    fallback(Reason::BudgetUnavailable)
                }
                Some(known) => fallback(known),
                None => {
                    ledger_unavailable(p, "/api/ai", &format!("unknown denial: {reason}"));
                    fallback(Reason::BudgetUnavailable)
                }
            };
        }
        Ok(other) => {
            let error = match other {
                LedgerReply::Error { error } => error,
                other => format!("unexpected reply: {}", other.status()),
            };
            ledger_unavailable(p, "/api/ai", &error);
            return fallback(Reason::BudgetUnavailable);
        }
        Err(error) => {
            ledger_unavailable(p, "/api/ai", &error.to_string());
            return fallback(Reason::BudgetUnavailable);
        }
    };

    // One 16 s budget from t0: the provider gets min(15 s, deadline − elapsed − margin).
    let start = p.now_ms();
    let timing = cfg.timing;
    let remaining = i128::from(timing.deadline_ms)
        - i128::from(start.saturating_sub(t0))
        - i128::from(timing.margin_ms);
    let deadline_bound = remaining < i128::from(timing.provider_cap_ms);
    let timer = remaining.clamp(0, i128::from(timing.provider_cap_ms));
    let result = if timer <= 0 {
        Err(ProviderError::Timer)
    } else {
        p.provider(body, millis(u64::try_from(timer).unwrap_or(0)))
            .await
    };
    let latency = i64::try_from(p.now_ms().saturating_sub(start)).unwrap_or(i64::MAX);
    let outcome = classify(result, deadline_bound, cfg);

    let (out, finish) = match outcome {
        Outcome::Answered(answer) => {
            p.log(
                Level::Info,
                "🤖 AI response completed",
                json!({
                    "question_id": qid,
                    "model": answer.model,
                    "cost_usd": usd(answer.nusd),
                    "usage": answer.usage,
                    "latency_ms": latency,
                }),
            );
            let finish = FinishOp {
                seq,
                outcome: "answered".into(),
                nusd: Some(answer.nusd),
                answer_id: Some(p.uuid()),
                answer: Some(sanitize_log_text(&answer.text)),
                answer_len: Some(i64::try_from(answer.text.chars().count()).unwrap_or(i64::MAX)),
                model: Some(answer.model.clone()),
                usage: Some(answer.usage.to_string()),
                latency_ms: Some(latency),
                error: None,
            };
            let out = Out::json(
                200,
                success_json(&answer.text, &answer.model, &answer.sources),
            );
            (out, finish)
        }
        Outcome::Failed {
            reason,
            class,
            message,
            nusd,
            usage,
        } => {
            match (reason, nusd) {
                (Reason::Timeout, _) => p.log(
                    Level::Warn,
                    "⏱️ AI timeout; reservation retained",
                    json!({ "question_id": qid }),
                ),
                (_, Some(cost)) => p.log(
                    Level::Warn,
                    "⚠️ AI failed; actual usage charged",
                    json!({ "question_id": qid, "error": message, "cost_usd": usd(cost) }),
                ),
                (_, None) => p.log(
                    Level::Warn,
                    "⚠️ AI failed; reservation retained for uncertain usage",
                    json!({ "question_id": qid, "error": message }),
                ),
            }
            let finish = FinishOp {
                seq,
                outcome: reason.code().into(),
                nusd,
                answer_id: None,
                answer: None,
                answer_len: None,
                model: None,
                usage: usage.map(|u| u.to_string()),
                latency_ms: Some(latency),
                error: Some(class),
            };
            (fallback(reason), finish)
        }
    };

    // Settle inline before responding; on failure respond anyway and retry once later.
    let finish = LedgerOp::Finish(finish);
    let settled = p
        .ledger(&finish, millis(cfg.timing.finish_timeout_ms))
        .await;
    if let Some(error) = settlement_error(settled) {
        p.log(
            Level::Warn,
            "🧾 Settlement failed; reservation kept",
            json!({ "question_id": qid, "seq": seq, "error": error, "retry": false }),
        );
        return Out {
            retry: Some(SettlementRetry {
                question_id: qid,
                seq,
                op: finish,
            }),
            ..out
        };
    }
    out
}

/// Maps the provider result to the response reason and the stored failure class (design 2.4).
fn classify(
    result: Result<ProviderHttp, ProviderError>,
    deadline_bound: bool,
    cfg: &Config,
) -> Outcome {
    let failed = |reason: Reason, class: &str, message: String| Outcome::Failed {
        reason,
        class: class.to_string(),
        message,
        nusd: None,
        usage: None,
    };
    match result {
        Ok(ProviderHttp { status, body }) if (200..300).contains(&status) => {
            match parse_reply(&body, &cfg.rates) {
                Ok(answer) => Outcome::Answered(answer),
                Err(ReplyError {
                    class,
                    message,
                    nusd,
                    usage,
                }) => Outcome::Failed {
                    reason: Reason::ProviderUnavailable,
                    class: class.to_string(),
                    message,
                    nusd,
                    usage,
                },
            }
        }
        Ok(ProviderHttp { status, .. }) => failed(
            Reason::ProviderUnavailable,
            &format!("http_{status}"),
            format!("Provider HTTP {status}"),
        ),
        Err(ProviderError::Timer) if deadline_bound => {
            failed(Reason::Timeout, "deadline", "AI deadline expired".into())
        }
        Err(ProviderError::Timer) => failed(
            Reason::ProviderUnavailable,
            "timeout",
            "Provider request timed out".into(),
        ),
        Err(ProviderError::TooLarge) => failed(
            Reason::ProviderUnavailable,
            "too_large",
            "Provider reply exceeds 64 KiB".into(),
        ),
        Err(ProviderError::Transport(error)) => {
            failed(Reason::ProviderUnavailable, "transport", error)
        }
    }
}

/// `None` when the settlement landed (`done`), else why it did not.
fn settlement_error(reply: Result<LedgerReply, PlatformError>) -> Option<String> {
    match reply {
        Ok(LedgerReply::Done) => None,
        Ok(LedgerReply::Error { error }) => Some(error),
        Ok(other) => Some(format!("unexpected reply: {}", other.status())),
        Err(error) => Some(error.to_string()),
    }
}

/// The single settlement retry, run by the glue in `wait_until` after `Timing::retry_delay_ms`.
/// Returns whether it landed; if not, the reservation stays and the lease frees the slot. Either
/// outcome is logged with the question id and the seq, like the inline failure.
pub async fn retry_finish(p: &impl Platform, cfg: &Config, retry: &SettlementRetry) -> bool {
    let reply = p
        .ledger(&retry.op, millis(cfg.timing.retry_timeout_ms))
        .await;
    match settlement_error(reply) {
        None => {
            p.log(
                Level::Info,
                "🧾 Settlement completed on retry",
                json!({ "question_id": retry.question_id, "seq": retry.seq }),
            );
            true
        }
        Some(error) => {
            p.log(
                Level::Warn,
                "🧾 Settlement failed; reservation kept",
                json!({
                    "question_id": retry.question_id,
                    "seq": retry.seq,
                    "error": error,
                    "retry": true,
                }),
            );
            false
        }
    }
}

// ---------------------------------------------------------------- /api/log/command

async fn command_log(p: &impl Platform, cfg: &Config, req: In<'_>) -> Out {
    // axum 0.7.9 `Json<CommandLogRequest>`: content type, body limit, JSON; then the handler.
    let payload: CommandLogRequest =
        match extract_json(req.content_type, req.content_length, req.body) {
            Ok(payload) => payload,
            Err(rejection) => return rejection.into_out(),
        };
    let trimmed = payload.command.trim();
    if !valid_command(trimmed) {
        return Out::empty(400);
    }
    let Some(mode) = command_mode(payload.mode.as_deref()) else {
        return Out::empty(400);
    };
    let ip = visitor(p, req.cf_connecting_ip, "/api/log/command");
    let op = LedgerOp::Command(CommandOp {
        command: sanitize_command(trimmed),
        command_len: i64::try_from(trimmed.chars().count()).unwrap_or(i64::MAX),
        mode: mode.to_string(),
        ip: ip.ip,
    });
    match p.ledger(&op, millis(cfg.timing.command_timeout_ms)).await {
        Ok(LedgerReply::Done) => Out::empty(204),
        Ok(LedgerReply::Refused { .. }) => Out::empty(429),
        other => {
            let error = match other {
                Ok(LedgerReply::Error { error }) => error,
                Ok(reply) => format!("unexpected reply: {}", reply.status()),
                Err(error) => error.to_string(),
            };
            p.log(
                Level::Warn,
                "📝 Failed to persist command log entry",
                json!({ "error": error }),
            );
            Out::empty(500)
        }
    }
}

// ---------------------------------------------------------------- /api/admin/*

fn admin_rejected(p: &impl Platform, path: &str) -> Out {
    log_throttled(
        p,
        Level::Warn,
        "🔐 Admin request rejected",
        json!({ "path": path }),
    );
    Out::not_found()
}

fn admin_json(status: u16, body: String) -> Out {
    Out::json(status, body).header("Cache-Control", NO_STORE.to_string())
}

fn admin_error(status: u16, code: &str) -> Out {
    admin_json(status, error_json(code))
}

/// DO error codes caused by the request rather than by the ledger's availability.
fn bad_input(code: &str) -> bool {
    !matches!(
        code,
        "storage" | "unsupported_op" | "invalid_op" | "encoding"
    )
}

/// The admin token, the JSON body: `None` (404) unless admin is enabled and the token matches.
fn admin_body(cfg: &Config, req: In<'_>) -> Option<Value> {
    let expected = cfg.admin_token.as_deref()?;
    if !json_content_type(req.content_type) {
        return None;
    }
    let bytes = match effective_body(req.content_length, req.body) {
        Body::Bytes(bytes) => bytes,
        Body::Empty | Body::TooLarge | Body::Unreadable(_) => return None,
    };
    let body: Value = serde_json::from_slice(&bytes).ok()?;
    let token = body.get("token")?.as_str()?;
    token_matches(expected, token).then_some(body)
}

async fn admin(p: &impl Platform, cfg: &Config, route: AdminRoute, req: In<'_>) -> Out {
    let path = req.path;
    let Some(body) = admin_body(cfg, req) else {
        return admin_rejected(p, path);
    };
    let timeout = millis(cfg.timing.admin_timeout_ms);
    let op = match route {
        AdminRoute::Stats => LedgerOp::Stats,
        AdminRoute::Rebuild => LedgerOp::Rebuild,
        AdminRoute::ImportClose => LedgerOp::ImportClose,
        AdminRoute::Import => {
            let Some(charges) = body.get("charges").and_then(Value::as_array) else {
                return admin_error(400, "missing_charges");
            };
            if charges.len() > IMPORT_MAX {
                return admin_error(400, "too_many_charges");
            }
            LedgerOp::Import(ImportOp {
                charges: charges.clone(),
            })
        }
        AdminRoute::Purge => {
            let table = body
                .get("table")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if table != "questions" && table != "commands" {
                return admin_error(400, "invalid_table");
            }
            let Some(before) = body.get("before").and_then(Value::as_i64) else {
                return admin_error(400, "invalid_before");
            };
            LedgerOp::Purge(PurgeOp {
                table: table.to_string(),
                before,
            })
        }
        AdminRoute::Export => {
            let name = body
                .get("table")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let Some(table) = Table::parse(name) else {
                return admin_error(400, "invalid_table");
            };
            let after = match body.get("after") {
                None | Some(Value::Null) => "0".to_string(),
                Some(Value::String(s)) => s.clone(),
                Some(Value::Number(n)) => n.to_string(),
                Some(_) => return admin_error(400, "invalid_cursor"),
            };
            if parse_cursor(table, &after).is_none() {
                return admin_error(400, "invalid_cursor");
            }
            let limit = match body.get("limit") {
                None | Some(Value::Null) => None,
                Some(value) => match value.as_i64() {
                    Some(limit) => Some(limit),
                    None => return admin_error(400, "invalid_limit"),
                },
            };
            let op = LedgerOp::Export(ExportOp {
                table: name.to_string(),
                after,
                limit,
            });
            return match p.ledger_page(&op, timeout).await {
                Ok(page) => {
                    let mut out = Out {
                        status: 200,
                        content_type: Some(NDJSON),
                        headers: vec![("Cache-Control", NO_STORE.to_string())],
                        body: OutBody::Bytes(page.body),
                        retry: None,
                    };
                    if let Some(next) = page.next_after {
                        out = out.header("X-Next-After", next);
                    }
                    out
                }
                Err(PlatformError::Rejected(code)) if bad_input(&code) => admin_error(400, &code),
                Err(error) => {
                    ledger_unavailable(p, path, &error.to_string());
                    admin_error(503, "ledger_unavailable")
                }
            };
        }
    };
    match p.ledger(&op, timeout).await {
        Ok(LedgerReply::Error { error }) if bad_input(&error) => admin_error(400, &error),
        Ok(LedgerReply::Error { error }) => {
            ledger_unavailable(p, path, &error);
            admin_error(503, "ledger_unavailable")
        }
        Ok(LedgerReply::Unknown) => {
            ledger_unavailable(p, path, "unexpected reply: unknown");
            admin_error(503, "ledger_unavailable")
        }
        Ok(reply) => {
            if let LedgerReply::Imported {
                inserted,
                updated,
                skipped,
            } = &reply
            {
                p.log(
                    Level::Info,
                    "📥 Ledger import chunk",
                    json!({ "inserted": inserted, "updated": updated, "skipped": skipped }),
                );
            }
            admin_json(200, serde_json::to_string(&reply).unwrap_or_default())
        }
        Err(error) => {
            ledger_unavailable(p, path, &error.to_string());
            admin_error(503, "ledger_unavailable")
        }
    }
}

#[cfg(test)]
mod tests;
