//! Worker ↔ Durable Object protocol: `POST https://ledger/op` with one JSON op, one JSON reply.
//!
//! Tolerant on purpose, so that a Worker and a Durable Object of different versions keep talking
//! during deploys and rollbacks: no `deny_unknown_fields`, every field added after version 1 gets
//! `#[serde(default)]`, an unknown op decodes to `Unsupported` and an unknown reply status to
//! `Unknown`.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Requests (internally tagged by `"op"`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum LedgerOp {
    Admit(AdmitOp),
    Finish(FinishOp),
    Command(CommandOp),
    Ping,
    Import(ImportOp),
    ImportClose,
    Export(ExportOp),
    Purge(PurgeOp),
    Stats,
    Rebuild,
    /// Any op this version does not know; answered `{"status":"error","error":"unsupported_op"}`.
    #[serde(other)]
    Unsupported,
}

impl LedgerOp {
    pub fn name(&self) -> &'static str {
        match self {
            LedgerOp::Admit(_) => "admit",
            LedgerOp::Finish(_) => "finish",
            LedgerOp::Command(_) => "command",
            LedgerOp::Ping => "ping",
            LedgerOp::Import(_) => "import",
            LedgerOp::ImportClose => "import_close",
            LedgerOp::Export(_) => "export",
            LedgerOp::Purge(_) => "purge",
            LedgerOp::Stats => "stats",
            LedgerOp::Rebuild => "rebuild",
            LedgerOp::Unsupported => "unsupported",
        }
    }
}

/// Admission of one `/api/ai` call (reservation, question row, IP hit, slot lease).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AdmitOp {
    /// UUID v4: the `question_id` of the logs.
    pub qid: String,
    /// Canonical visitor IP (stored).
    pub ip: String,
    /// IPv4 address or IPv6 /64 prefix (limiter key).
    pub ip_key: String,
    /// Reservation in nano-USD.
    pub nusd: i64,
    /// `sanitize(question)`.
    pub question: String,
    /// Characters of the raw question.
    pub question_len: i64,
}

/// Settlement of an admitted call.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FinishOp {
    pub seq: i64,
    /// `answered`, `provider_unavailable` or `timeout`.
    pub outcome: String,
    /// Actual cost in nano-USD when known; `null` keeps the reservation.
    #[serde(default)]
    pub nusd: Option<i64>,
    #[serde(default)]
    pub answer_id: Option<String>,
    /// `sanitize(trimmed answer)`.
    #[serde(default)]
    pub answer: Option<String>,
    #[serde(default)]
    pub answer_len: Option<i64>,
    /// Model reported by the provider.
    #[serde(default)]
    pub model: Option<String>,
    /// Raw provider `usage` JSON (compact).
    #[serde(default)]
    pub usage: Option<String>,
    #[serde(default)]
    pub latency_ms: Option<i64>,
    /// Failure class.
    #[serde(default)]
    pub error: Option<String>,
}

/// One command telemetry line.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CommandOp {
    /// `sanitize(trimmed command)` cut at 200 characters.
    pub command: String,
    /// Characters of the trimmed command.
    pub command_len: i64,
    /// `classic` or `ai` (already trimmed and checked).
    pub mode: String,
    pub ip: String,
}

/// A chunk (≤ 25, `IMPORT_MAX`) of VPS ledger charges `{"id","at","usd"}`, validated entry by
/// entry.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ImportOp {
    pub charges: Vec<Value>,
}

/// One export page.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExportOp {
    /// `questions`, `commands` or `ledger`.
    pub table: String,
    /// Cursor: `"0"` to start, then the `X-Next-After` of the previous page.
    #[serde(default = "start_cursor")]
    pub after: String,
    /// Rows per page (clamped to the table's maximum).
    #[serde(default)]
    pub limit: Option<i64>,
}

fn start_cursor() -> String {
    "0".to_string()
}

/// One bounded deletion batch.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PurgeOp {
    /// `questions` or `commands`.
    pub table: String,
    /// Unix seconds: rows with `at <= before` (questions: never inside the 30-day window).
    pub before: i64,
}

/// Replies (internally tagged by `"status"`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum LedgerReply {
    Admitted {
        seq: i64,
    },
    Denied {
        /// `request_limit`, `busy`, `spending_limit` or `budget_unavailable`.
        reason: String,
        /// Internal detail, for logs only.
        #[serde(default)]
        detail: Option<String>,
    },
    Done,
    Refused {
        /// `window_full` or `daily_cap`.
        reason: String,
    },
    Pong {
        ledger_ok: bool,
        text_full: bool,
        db_bytes: i64,
    },
    Imported {
        inserted: i64,
        updated: i64,
        skipped: i64,
    },
    Purged {
        deleted: i64,
        more: bool,
        /// Today's delete budget (`LOG_DELETE_DAILY_ROWS`) stops this purge until 00:00 UTC.
        /// Sent only when true.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        budget_exhausted: bool,
    },
    Stats(Box<StatsReply>),
    Error {
        error: String,
    },
    /// Any status this version does not know; treated as a transport error.
    #[serde(other)]
    Unknown,
}

impl LedgerReply {
    pub fn denied(reason: &str, detail: &str) -> LedgerReply {
        LedgerReply::Denied {
            reason: reason.to_string(),
            detail: Some(detail.to_string()),
        }
    }

    pub fn error(code: impl Into<String>) -> LedgerReply {
        LedgerReply::Error { error: code.into() }
    }

    /// The `status` tag, for logs.
    pub fn status(&self) -> &'static str {
        match self {
            LedgerReply::Admitted { .. } => "admitted",
            LedgerReply::Denied { .. } => "denied",
            LedgerReply::Done => "done",
            LedgerReply::Refused { .. } => "refused",
            LedgerReply::Pong { .. } => "pong",
            LedgerReply::Imported { .. } => "imported",
            LedgerReply::Purged { .. } => "purged",
            LedgerReply::Stats(_) => "stats",
            LedgerReply::Error { .. } => "error",
            LedgerReply::Unknown => "unknown",
        }
    }
}

/// Amounts per spend window, in USD (`null` for a budget that is not a finite number).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowsUsd {
    pub minute: Option<f64>,
    pub hour: Option<f64>,
    pub day: Option<f64>,
    pub month: Option<f64>,
}

impl WindowsUsd {
    /// From values in window order (minute, hour, day, month); non-finite values become `None`.
    pub fn from_values(values: [f64; 4]) -> WindowsUsd {
        let [minute, hour, day, month] = values.map(|v| v.is_finite().then_some(v));
        WindowsUsd {
            minute,
            hour,
            day,
            month,
        }
    }
}

/// A slot lease: question seq and lease expiry (unix seconds).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lease {
    pub seq: i64,
    pub lease_until: i64,
}

/// `POST /api/admin/stats`: O(1) aggregates plus bounded counts; never an unbounded `COUNT(*)`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StatsReply {
    pub schema_version: i64,
    pub min_reader_version: i64,
    pub reader_version: i64,
    pub valid: bool,
    pub invalid_reason: Option<String>,
    pub budget_valid: bool,
    pub sums_usd: WindowsUsd,
    pub limits_usd: WindowsUsd,
    pub inflight: Vec<Lease>,
    pub calls_today: i64,
    pub daily_max_calls: i64,
    pub cmd_today: i64,
    pub command_daily_cap: i64,
    pub ip_count: i64,
    pub next_seq: i64,
    pub cmd_next_seq: i64,
    /// `MAX(seq) − MIN(seq) + 1`: approximate once rows are purged.
    pub questions_rows: i64,
    pub commands_rows: i64,
    pub db_bytes: i64,
    pub text_bytes: i64,
    pub text_budget_bytes: i64,
    pub text_dropped: i64,
    pub retention_days: i64,
    /// Rows written today by log deletes (purge and retention), and the daily budget.
    pub delete_rows_today: i64,
    pub delete_daily_rows: i64,
    pub maintained_at: i64,
    pub import_closed: bool,
    pub imported_rows: i64,
    pub unknown_ip_questions_24h: i64,
    pub unknown_ip_commands_last_10k: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip_op(op: LedgerOp) {
        let text = serde_json::to_string(&op).unwrap();
        assert_eq!(
            serde_json::from_str::<LedgerOp>(&text).unwrap(),
            op,
            "{text}"
        );
    }

    fn round_trip_reply(reply: LedgerReply) {
        let text = serde_json::to_string(&reply).unwrap();
        assert_eq!(
            serde_json::from_str::<LedgerReply>(&text).unwrap(),
            reply,
            "{text}"
        );
    }

    #[test]
    fn every_op_and_reply_round_trips() {
        round_trip_op(LedgerOp::Admit(AdmitOp {
            qid: "8c0b".into(),
            ip: "203.0.113.7".into(),
            ip_key: "203.0.113.7".into(),
            nusd: 2_532_000,
            question: "Micro Mages?".into(),
            question_len: 12,
        }));
        round_trip_op(LedgerOp::Finish(FinishOp {
            seq: 42,
            outcome: "answered".into(),
            nusd: Some(20_000),
            answer_id: Some("1f5e".into()),
            answer: Some("…".into()),
            answer_len: Some(312),
            model: Some("gpt-6-luna".into()),
            usage: Some("{\"prompt_tokens\":1}".into()),
            latency_ms: Some(812),
            error: None,
        }));
        round_trip_op(LedgerOp::Command(CommandOp {
            command: "projects".into(),
            command_len: 8,
            mode: "classic".into(),
            ip: "203.0.113.7".into(),
        }));
        round_trip_op(LedgerOp::Ping);
        round_trip_op(LedgerOp::Import(ImportOp {
            charges: vec![serde_json::json!({"id":"a","at":1,"usd":0.5})],
        }));
        round_trip_op(LedgerOp::ImportClose);
        round_trip_op(LedgerOp::Export(ExportOp {
            table: "ledger".into(),
            after: "5:7".into(),
            limit: Some(10),
        }));
        round_trip_op(LedgerOp::Purge(PurgeOp {
            table: "commands".into(),
            before: 1_760_000_000,
        }));
        round_trip_op(LedgerOp::Stats);
        round_trip_op(LedgerOp::Rebuild);
        round_trip_reply(LedgerReply::Admitted { seq: 42 });
        round_trip_reply(LedgerReply::denied("busy", "slots"));
        round_trip_reply(LedgerReply::Done);
        round_trip_reply(LedgerReply::Refused {
            reason: "window_full".into(),
        });
        round_trip_reply(LedgerReply::Pong {
            ledger_ok: true,
            text_full: false,
            db_bytes: 1_048_576,
        });
        round_trip_reply(LedgerReply::Imported {
            inserted: 250,
            updated: 0,
            skipped: 3,
        });
        round_trip_reply(LedgerReply::Purged {
            deleted: 1_000,
            more: true,
            budget_exhausted: false,
        });
        round_trip_reply(LedgerReply::Purged {
            deleted: 400,
            more: true,
            budget_exhausted: true,
        });
        // The flag goes on the wire only when set, so version-1 readers see the same reply.
        assert_eq!(
            serde_json::to_string(&LedgerReply::Purged {
                deleted: 3,
                more: false,
                budget_exhausted: false
            })
            .unwrap(),
            r#"{"status":"purged","deleted":3,"more":false}"#
        );
        assert_eq!(
            serde_json::to_string(&LedgerReply::Purged {
                deleted: 0,
                more: true,
                budget_exhausted: true
            })
            .unwrap(),
            r#"{"status":"purged","deleted":0,"more":true,"budget_exhausted":true}"#
        );
        round_trip_reply(LedgerReply::Stats(Box::new(StatsReply {
            valid: true,
            inflight: vec![Lease {
                seq: 1,
                lease_until: 20,
            }],
            ..StatsReply::default()
        })));
        round_trip_reply(LedgerReply::error("unknown_reservation"));
    }

    /// Version-1 wire fixtures (design 3.13): what a Worker or Durable Object of this version sends.
    #[test]
    fn version_1_fixtures_decode() {
        let ops = [
            r#"{"op":"admit","qid":"8c0b6f0e","ip":"203.0.113.7","ip_key":"203.0.113.7","nusd":2532000,"question":"Micro Mages?","question_len":12}"#,
            r#"{"op":"finish","seq":42,"outcome":"answered","nusd":20000,"answer_id":"1f5e","answer":"…","answer_len":312,"model":"gpt-6-luna","usage":"{\"prompt_tokens\":100}","latency_ms":812,"error":null}"#,
            r#"{"op":"finish","seq":43,"outcome":"timeout"}"#,
            r#"{"op":"command","command":"projects","command_len":8,"mode":"classic","ip":"203.0.113.7"}"#,
            r#"{"op":"ping"}"#,
            r#"{"op":"import","charges":[{"id":"x","at":1764000000,"usd":0.000873}]}"#,
            r#"{"op":"import_close"}"#,
            r#"{"op":"export","table":"questions","after":"0","limit":200}"#,
            r#"{"op":"export","table":"commands"}"#,
            r#"{"op":"purge","table":"commands","before":1760000000}"#,
            r#"{"op":"stats"}"#,
            r#"{"op":"rebuild"}"#,
        ];
        let names: Vec<&str> = ops
            .iter()
            .map(|t| serde_json::from_str::<LedgerOp>(t).unwrap().name())
            .collect();
        assert_eq!(
            names,
            [
                "admit",
                "finish",
                "finish",
                "command",
                "ping",
                "import",
                "import_close",
                "export",
                "export",
                "purge",
                "stats",
                "rebuild"
            ]
        );
        let replies = [
            (r#"{"status":"admitted","seq":42}"#, "admitted"),
            (
                r#"{"status":"denied","reason":"busy","detail":"slots"}"#,
                "denied",
            ),
            (r#"{"status":"denied","reason":"busy"}"#, "denied"),
            (r#"{"status":"done"}"#, "done"),
            (r#"{"status":"refused","reason":"window_full"}"#, "refused"),
            (
                r#"{"status":"pong","ledger_ok":true,"text_full":false,"db_bytes":1048576}"#,
                "pong",
            ),
            (
                r#"{"status":"imported","inserted":250,"updated":0,"skipped":3}"#,
                "imported",
            ),
            (
                r#"{"status":"purged","deleted":1000,"more":true}"#,
                "purged",
            ),
            (r#"{"status":"stats","valid":true,"ip_count":3}"#, "stats"),
            (
                r#"{"status":"error","error":"unknown_reservation"}"#,
                "error",
            ),
        ];
        for (text, status) in replies {
            assert_eq!(
                serde_json::from_str::<LedgerReply>(text).unwrap().status(),
                status,
                "{text}"
            );
        }
    }

    #[test]
    fn tolerant_decoding() {
        // Unknown op and unknown reply status.
        assert_eq!(
            serde_json::from_str::<LedgerOp>(r#"{"op":"compact","x":1}"#).unwrap(),
            LedgerOp::Unsupported
        );
        assert_eq!(
            serde_json::from_str::<LedgerReply>(r#"{"status":"throttled","retry":3}"#).unwrap(),
            LedgerReply::Unknown
        );
        // Fields added by a later version are ignored.
        let op: LedgerOp = serde_json::from_str(r#"{"op":"ping","trace":"abc"}"#).unwrap();
        assert_eq!(op, LedgerOp::Ping);
        let reply: LedgerReply =
            serde_json::from_str(r#"{"status":"admitted","seq":7,"region":"weur"}"#).unwrap();
        assert_eq!(reply, LedgerReply::Admitted { seq: 7 });
        // Malformed known ops are errors (answered `invalid_op` by the Durable Object).
        assert!(serde_json::from_str::<LedgerOp>(r#"{"op":"admit","qid":"x"}"#).is_err());
        assert!(serde_json::from_str::<LedgerOp>(r#"{"seq":1}"#).is_err());
        // Shapes on the wire.
        assert_eq!(
            serde_json::to_string(&LedgerOp::Ping).unwrap(),
            r#"{"op":"ping"}"#
        );
        assert_eq!(
            serde_json::to_string(&LedgerReply::Done).unwrap(),
            r#"{"status":"done"}"#
        );
    }
}
