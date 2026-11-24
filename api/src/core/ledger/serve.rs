//! The Durable Object's request handling, independent of the runtime: decode one op, run it on
//! the cached `Book` inside a storage transaction, run the lazy maintenance after admits and
//! commands, encode the reply. The glue only provides `LedgerHost` (cached book, store,
//! `transactionSync`) and a log sink.

use serde_json::{json, Value};

use super::protocol::{LedgerOp, LedgerReply};
use super::store::Store;
use super::{Book, ExportOutcome, LedgerError, LogEvent};
use crate::core::config::LedgerConfig;
use crate::core::http::{APPLICATION_JSON, NDJSON};
use crate::core::logline::{with_suppressed, Level, Throttle};

/// Owner of the cached `Book` and of the storage.
pub trait LedgerHost {
    type Store: Store;

    /// Runs `work` on the cached book (loaded first with `Book::load` when absent) inside one
    /// storage transaction. When `work` or the commit fails, every statement is rolled back, the
    /// cached book is dropped and the error returned.
    fn transact<T: 'static>(
        &self,
        now: i64,
        work: impl FnOnce(&mut Book, &mut Self::Store) -> Result<T, LedgerError> + 'static,
    ) -> Result<T, LedgerError>;
}

/// The Durable Object's HTTP reply (always status 200).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DoResponse {
    pub content_type: &'static str,
    /// `X-Next-After` of an export page.
    pub next_after: Option<String>,
    pub body: Vec<u8>,
}

impl DoResponse {
    fn json(reply: &LedgerReply) -> DoResponse {
        DoResponse {
            content_type: APPLICATION_JSON,
            next_after: None,
            body: serde_json::to_vec(reply)
                .unwrap_or_else(|_| br#"{"status":"error","error":"encoding"}"#.to_vec()),
        }
    }
}

/// Where the Durable Object's log lines go.
pub struct DoLog<'a> {
    pub sink: &'a dyn Fn(Level, &'static str, Value),
    pub throttle: &'a Throttle,
    pub now_ms: u64,
}

impl DoLog<'_> {
    fn emit(&self, event: LogEvent) {
        if event.throttled {
            if let Some(suppressed) = self.throttle.admit(event.msg, self.now_ms) {
                (self.sink)(
                    event.level,
                    event.msg,
                    with_suppressed(event.fields, suppressed),
                );
            }
        } else {
            (self.sink)(event.level, event.msg, event.fields);
        }
    }

    fn emit_all(&self, events: Vec<LogEvent>) {
        for event in events {
            self.emit(event);
        }
    }
}

/// Handles one op body at `now_ms` (the Durable Object's clock).
pub fn serve<H: LedgerHost>(
    host: &H,
    cfg: LedgerConfig,
    now_ms: u64,
    body: &[u8],
    log: &DoLog<'_>,
) -> DoResponse {
    let now = i64::try_from(now_ms / 1000).unwrap_or(i64::MAX);
    let op: LedgerOp = match serde_json::from_slice(body) {
        Ok(op) => op,
        Err(_) => return DoResponse::json(&LedgerReply::error("invalid_op")),
    };
    let name = op.name();
    let failed = |error: &LedgerError| {
        (log.sink)(
            Level::Warn,
            "🧾 Ledger transaction failed",
            json!({"op": name, "error": error.to_string()}),
        );
    };

    let maintain_after = matches!(op, LedgerOp::Admit(_) | LedgerOp::Command(_));
    let outcome: Result<LedgerReply, LedgerError> = match op {
        LedgerOp::Admit(admit) => match run(host, log, now, move |book, store| {
            book.admit(store, &cfg, now, &admit)
        }) {
            Err(error) => {
                failed(&error);
                // A failed admission commit denies the call, as a failed ledger write did.
                return finish_with_maintenance(
                    host,
                    cfg,
                    now,
                    log,
                    false,
                    LedgerReply::denied("spending_limit", "ledger_write_failed"),
                );
            }
            ok => ok,
        },
        LedgerOp::Finish(finish) => run(host, log, now, move |book, store| {
            book.finish(store, &cfg, now, &finish)
        }),
        LedgerOp::Command(command) => run(host, log, now, move |book, store| {
            book.command(store, &cfg, now, &command)
        }),
        LedgerOp::Ping => run(host, log, now, move |book, store| book.ping(store, &cfg)),
        LedgerOp::Import(import) => run(host, log, now, move |book, store| {
            book.import(store, now, &import)
        }),
        LedgerOp::ImportClose => run(host, log, now, |book, store| book.import_close(store)),
        LedgerOp::Purge(purge) => run(host, log, now, move |book, store| {
            book.purge(store, &cfg, now, &purge)
        }),
        LedgerOp::Stats => run(host, log, now, move |book, store| {
            book.stats(store, &cfg, now)
        }),
        LedgerOp::Rebuild => run(host, log, now, move |book, store| book.rebuild(store, now)),
        LedgerOp::Export(export) => {
            return match run(host, log, now, move |book, store| {
                book.export(store, now, &export)
            }) {
                Ok(ExportOutcome::Page { body, next_after }) => DoResponse {
                    content_type: NDJSON,
                    next_after,
                    body,
                },
                Ok(ExportOutcome::Rejected(code)) => DoResponse::json(&LedgerReply::error(code)),
                Err(error) => {
                    failed(&error);
                    DoResponse::json(&LedgerReply::error(error.code()))
                }
            }
        }
        LedgerOp::Unsupported => Ok(LedgerReply::error("unsupported_op")),
    };
    match outcome {
        Ok(reply) => finish_with_maintenance(host, cfg, now, log, maintain_after, reply),
        Err(error) => {
            failed(&error);
            DoResponse::json(&LedgerReply::error(error.code()))
        }
    }
}

/// Runs one op in a transaction and emits the log lines it produced.
fn run<H: LedgerHost, T: 'static>(
    host: &H,
    log: &DoLog<'_>,
    now: i64,
    work: impl FnOnce(&mut Book, &mut H::Store) -> Result<T, LedgerError> + 'static,
) -> Result<T, LedgerError> {
    let (value, events) = host.transact(now, move |book, store| {
        let value = work(book, store)?;
        Ok((value, book.take_events()))
    })?;
    log.emit_all(events);
    Ok(value)
}

/// After an admit or command (whatever its result): one bounded maintenance pass when due, in
/// its own transaction. Its failure is logged and never changes the op's reply.
fn finish_with_maintenance<H: LedgerHost>(
    host: &H,
    cfg: LedgerConfig,
    now: i64,
    log: &DoLog<'_>,
    maintain: bool,
    reply: LedgerReply,
) -> DoResponse {
    if maintain {
        match run(host, log, now, move |book, store| {
            book.maintain(store, &cfg, now)
        }) {
            Ok(Some(report)) => (log.sink)(
                Level::Info,
                "🧹 Ledger maintenance",
                json!({
                    "deleted_ips": report.ips,
                    "deleted_questions": report.questions,
                    "deleted_commands": report.commands,
                    "text_bytes_freed": report.text_bytes_freed,
                    "backlog": report.backlog,
                    "delete_budget_left": report.delete_budget_left,
                }),
            ),
            Ok(None) => {}
            Err(error) => (log.sink)(
                Level::Warn,
                "🧹 Ledger maintenance failed",
                json!({"error": error.to_string()}),
            ),
        }
    }
    DoResponse::json(&reply)
}
