//! The `Ledger` Durable Object's state: spend ledger, per-IP windows, provider slots, command
//! window and counters, and the question, answer and command logs (design section 3).
//!
//! `Book` is the in-memory cache of the `meta` row. Its methods take the store and run inside one
//! transaction opened by the caller (`serve::LedgerHost::transact`): `transactionSync` in the
//! Durable Object, `BEGIN … COMMIT` in native tests. When a transaction fails, the caller drops
//! the `Book` and the next op reloads it from storage.

pub mod export;
pub mod ipwin;
pub mod meta;
pub mod protocol;
pub mod serve;
pub mod sql;
#[cfg(test)]
pub mod sqlite_store;
pub mod store;

use std::collections::VecDeque;
use std::fmt;

use serde_json::{json, Value};

use self::export::{
    command_line, exportable, format_cursor, ledger_line, parse_cursor, question_lines, Cursor,
    QuestionRow, Table, PAGE_BYTES, STATE_ANSWERED, STATE_INFLIGHT, STATE_PROVIDER_UNAVAILABLE,
    STATE_TIMEOUT,
};
use self::ipwin::{format_hits, full_window, parse_hits, IP_CAP, IP_HISTORY, IP_PRUNE_EVERY};
use self::meta::{
    format_inflight, parse_inflight, Meta, MONTH, READER_VERSION, SCHEMA_VERSION, WINDOWS,
    WINDOW_NAMES,
};
use self::protocol::{
    AdmitOp, CommandOp, ExportOp, FinishOp, ImportOp, Lease, LedgerReply, PurgeOp, StatsReply,
    WindowsUsd,
};
use self::store::{col_i64, col_text, query_count, query_i64, SqlValue, Store, StoreError};
use super::config::LedgerConfig;
use super::logline::Level;
use super::money::{usd, MAX_NUSD};
use super::time::{day_number, DAY};

/// A provider slot is leased for 20 s from admission (DO clock).
pub const LEASE_SECS: i64 = 20;
/// Provider calls in flight at the same time.
pub const SLOTS: usize = 2;
/// Global command window: 120 lines per 60 s.
pub const COMMAND_WINDOW_SECS: i64 = 60;
pub const COMMAND_WINDOW_MAX: usize = 120;
/// Lazy maintenance: at most once per hour, 250 rows per table per pass, 60 s while a backlog
/// remains.
pub const MAINTENANCE_EVERY: i64 = 3_600;
pub const MAINTENANCE_BACKLOG_DELAY: i64 = 60;
pub const MAINTENANCE_BATCH: i64 = 250;
/// Rows deleted per admin purge call.
pub const PURGE_BATCH: i64 = 1_000;
/// Charges per import chunk.
pub const IMPORT_MAX: usize = 250;
/// `text_full` once less than this is left of the text budget.
pub const TEXT_FULL_MARGIN: i64 = 65_536;
/// Rebuild counts today's commands among this many recent seqs.
pub const CMD_TODAY_SCAN: i64 = 20_000;
/// Stats counts unknown-IP commands among this many recent seqs.
pub const UNKNOWN_IP_COMMAND_SCAN: i64 = 10_000;
/// Longest accepted VPS charge id.
pub const MAX_CHARGE_ID_CHARS: usize = 64;

/// Why an op could not complete. Either way the caller rolls the transaction back and drops the
/// cached `Book`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LedgerError {
    /// A statement or the commit failed.
    Store(String),
    /// A consistency check failed after writes had started (for example a sum overflow).
    Abort(&'static str),
}

impl LedgerError {
    /// The code of the `error` reply.
    pub fn code(&self) -> String {
        match self {
            LedgerError::Store(_) => "storage".to_string(),
            LedgerError::Abort(code) => (*code).to_string(),
        }
    }
}

impl fmt::Display for LedgerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LedgerError::Store(error) => write!(f, "storage: {error}"),
            LedgerError::Abort(code) => f.write_str(code),
        }
    }
}

impl From<StoreError> for LedgerError {
    fn from(error: StoreError) -> LedgerError {
        LedgerError::Store(error.0)
    }
}

/// A log line produced inside a transaction, emitted by the caller afterwards.
#[derive(Clone, Debug, PartialEq)]
pub struct LogEvent {
    pub level: Level,
    pub msg: &'static str,
    pub fields: Value,
    /// At most once per 60 s per isolate.
    pub throttled: bool,
}

/// Result of a maintenance pass.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Maintenance {
    pub ips: i64,
    pub questions: i64,
    pub commands: i64,
    pub text_bytes_freed: i64,
    /// A batch was full: the next pass comes 60 s later.
    pub backlog: bool,
}

/// An export op's result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExportOutcome {
    /// NDJSON body, and the cursor of the next page while more remain.
    Page {
        body: Vec<u8>,
        next_after: Option<String>,
    },
    /// Invalid input (`invalid_table`, `invalid_cursor`).
    Rejected(&'static str),
}

struct Charge {
    id: String,
    at: i64,
    nusd: i64,
}

/// In-memory state of the ledger, loaded on the first op after a wake.
#[derive(Clone, Debug)]
pub struct Book {
    meta: Meta,
    /// Slot leases `(seq, lease_until)`, parsed from `meta.inflight`.
    inflight: Vec<(i64, i64)>,
    /// False when the stored state cannot be trusted: every admission is denied
    /// (`budget_unavailable`), `meta` is never written, commands keep working. Repair: `rebuild`.
    valid: bool,
    invalid_reason: Option<&'static str>,
    /// Command timestamps of the last 60 s, loaded on the first command op.
    cmd_window: Option<VecDeque<i64>>,
    events: Vec<LogEvent>,
}

impl Book {
    /// Loads and validates `meta`. Missing meta with no question row: defaults, written by the
    /// first commit. Missing or untrusted meta otherwise: invalid.
    pub fn load(store: &mut impl Store, now: i64) -> Result<Book, LedgerError> {
        let rows = store.exec(sql::META_SELECT, &[])?;
        let (meta, problem, missing) = match rows.first() {
            Some(row) => match Meta::from_row(row) {
                Some(meta) => {
                    let problem = meta.problem();
                    (meta, problem, false)
                }
                None => (Meta::fresh(now), Some("meta_unreadable"), true),
            },
            None => {
                let any = !store.exec(sql::QUESTIONS_ANY, &[])?.is_empty();
                (Meta::fresh(now), any.then_some("meta_missing"), true)
            }
        };
        let valid = problem.is_none();
        let inflight = if valid {
            parse_inflight(&meta.inflight).unwrap_or_default()
        } else {
            Vec::new()
        };
        let mut book = Book {
            meta,
            inflight,
            valid,
            invalid_reason: problem,
            cmd_window: None,
            events: Vec::new(),
        };
        if missing || !valid {
            // Command rows must never reuse a seq, and an untrusted meta row is never written.
            let next =
                query_i64(store, sql::COMMANDS_MAX_SEQ, &[])?.map_or(1, |m| m.saturating_add(1));
            book.meta.cmd_next_seq = book.meta.cmd_next_seq.max(next);
            let newest = newest_command_at(store)?.unwrap_or(0);
            book.meta.cmd_last_at = book.meta.cmd_last_at.max(newest);
        }
        Ok(book)
    }

    pub fn meta(&self) -> &Meta {
        &self.meta
    }

    pub fn valid(&self) -> bool {
        self.valid
    }

    pub fn invalid_reason(&self) -> Option<&'static str> {
        self.invalid_reason
    }

    pub fn inflight(&self) -> &[(i64, i64)] {
        &self.inflight
    }

    /// Log lines produced since the last call.
    pub fn take_events(&mut self) -> Vec<LogEvent> {
        std::mem::take(&mut self.events)
    }

    fn invalidate(&mut self, reason: &'static str) {
        self.valid = false;
        self.invalid_reason = Some(reason);
    }

    /// Writes `meta` (one row), unless the book is invalid.
    fn save_meta(&mut self, store: &mut impl Store) -> Result<(), LedgerError> {
        if !self.valid {
            return Ok(());
        }
        self.meta.inflight = format_inflight(&self.inflight);
        store.exec(sql::META_UPSERT, &self.meta.to_params())?;
        Ok(())
    }

    /// Moves every window edge to `now − W`, subtracting the charges that left it (each row is
    /// read once per window over its life, from the covering index). Writes nothing.
    fn advance(&mut self, store: &mut impl Store, now: i64) -> Result<(), LedgerError> {
        for ((edge, sum), window) in self
            .meta
            .edges
            .iter_mut()
            .zip(self.meta.sums.iter_mut())
            .zip(WINDOWS)
        {
            let new_edge = now.saturating_sub(window);
            if new_edge > *edge {
                let gone = query_count(store, sql::SUM_RANGE, &[(*edge).into(), new_edge.into()])?;
                *sum = sum
                    .checked_sub(gone)
                    .ok_or(LedgerError::Abort("overflow"))?;
                *edge = new_edge;
            }
        }
        if self.meta.sums.iter().any(|s| *s < 0) {
            self.invalidate("negative_sum");
        }
        Ok(())
    }

    /// Adds `delta` to every window that counts a row admitted at `at` (checked).
    fn shift_sums(&mut self, at: i64, delta: i64) -> Result<(), LedgerError> {
        for (sum, edge) in self.meta.sums.iter_mut().zip(self.meta.edges) {
            if at > edge {
                *sum = sum
                    .checked_add(delta)
                    .ok_or(LedgerError::Abort("overflow"))?;
            }
        }
        if self.meta.sums.iter().any(|s| *s < 0) {
            self.invalidate("negative_sum");
        }
        Ok(())
    }

    /// Whether `text` fits the text budget; counts its bytes, or a dropped row.
    fn keep_text(&mut self, cfg: &LedgerConfig, text: &str) -> bool {
        let bytes = i64::try_from(text.len()).unwrap_or(i64::MAX);
        if self.meta.text_bytes.saturating_add(bytes) <= cfg.text_budget_bytes {
            self.meta.text_bytes = self.meta.text_bytes.saturating_add(bytes);
            true
        } else {
            self.meta.text_dropped = self.meta.text_dropped.saturating_add(1);
            self.events.push(LogEvent {
                level: Level::Warn,
                msg: "💾 Ledger text budget reached",
                fields: json!({"text_bytes": self.meta.text_bytes, "budget": cfg.text_budget_bytes}),
                throttled: true,
            });
            false
        }
    }

    fn text_full(&self, cfg: &LedgerConfig) -> bool {
        self.meta.text_bytes.saturating_add(TEXT_FULL_MARGIN) > cfg.text_budget_bytes
    }

    fn deny(
        &mut self,
        store: &mut impl Store,
        dirty: bool,
        reason: &str,
        detail: &str,
    ) -> Result<LedgerReply, LedgerError> {
        if dirty {
            self.save_meta(store)?;
        }
        Ok(LedgerReply::denied(reason, detail))
    }

    /// Admission (design 3.6), checks in the spec's order: ledger → IP (and the 4,096-key cap) →
    /// slots → spend. Only a full admission writes: the question row (reservation), the IP hit,
    /// the slot lease and `meta`.
    pub fn admit(
        &mut self,
        store: &mut impl Store,
        cfg: &LedgerConfig,
        now: i64,
        op: &AdmitOp,
    ) -> Result<LedgerReply, LedgerError> {
        if !self.valid {
            return Ok(LedgerReply::denied("budget_unavailable", "invalid_book"));
        }
        let Some(limits) = cfg.limits else {
            return Ok(LedgerReply::denied("budget_unavailable", "invalid_budget"));
        };
        self.advance(store, now)?;
        if !self.valid {
            return Ok(LedgerReply::denied("budget_unavailable", "invalid_book"));
        }
        let day = day_number(now);
        if self.meta.calls_day != day {
            self.meta.calls_day = day;
            self.meta.calls_today = 0;
        }

        let stored = store
            .exec(sql::IP_HITS, &[op.ip_key.as_str().into()])?
            .into_iter()
            .next()
            .and_then(|row| row.into_iter().next())
            .map(|v| v.into_text().unwrap_or_default());
        let is_new = stored.is_none();
        let mut hits = parse_hits(stored.as_deref().unwrap_or_default(), now);
        let mut dirty = false;
        if is_new && self.meta.ip_count >= IP_CAP {
            if now.saturating_sub(self.meta.ips_pruned_at) >= IP_PRUNE_EVERY {
                let deleted = store
                    .exec(sql::IP_PRUNE, &[(now - IP_HISTORY).into()])?
                    .len();
                let deleted = i64::try_from(deleted).unwrap_or(i64::MAX);
                self.meta.ip_count = self.meta.ip_count.saturating_sub(deleted).max(0);
                self.meta.ips_pruned_at = now;
                dirty = true;
            }
            if self.meta.ip_count >= IP_CAP {
                return self.deny(store, dirty, "request_limit", "traffic_limit");
            }
        }
        if let Some(window) = full_window(&hits, now) {
            let detail = format!("ip_window_{window}s");
            return self.deny(store, dirty, "request_limit", &detail);
        }
        self.inflight.retain(|&(_, lease)| lease > now);
        if self.inflight.len() >= SLOTS {
            return self.deny(store, dirty, "busy", "slots");
        }
        if op.nusd <= 0 || op.nusd > MAX_NUSD {
            return self.deny(store, dirty, "spending_limit", "invalid_reservation");
        }
        if self.meta.calls_today >= cfg.daily_max_calls {
            return self.deny(store, dirty, "spending_limit", "daily_call_cap");
        }
        let mut sums = self.meta.sums;
        for ((sum, limit), name) in sums.iter_mut().zip(limits).zip(WINDOW_NAMES) {
            match sum.checked_add(op.nusd) {
                None => return self.deny(store, dirty, "spending_limit", "overflow"),
                Some(total) if total > limit => {
                    return self.deny(store, dirty, "spending_limit", name)
                }
                Some(total) => *sum = total,
            }
        }

        let text = self
            .keep_text(cfg, &op.question)
            .then_some(op.question.as_str());
        let seq = self.meta.next_seq;
        let lease = now.saturating_add(LEASE_SECS);
        store.exec(
            sql::QUESTION_INSERT,
            &[
                seq.into(),
                op.qid.as_str().into(),
                now.into(),
                op.nusd.into(),
                lease.into(),
                op.ip.as_str().into(),
                text.into(),
                op.question_len.into(),
            ],
        )?;
        hits.push(now);
        store.exec(
            sql::IP_UPSERT,
            &[
                op.ip_key.as_str().into(),
                format_hits(&hits).into(),
                now.into(),
            ],
        )?;
        if is_new {
            self.meta.ip_count = self.meta.ip_count.saturating_add(1);
        }
        self.meta.next_seq = seq.saturating_add(1);
        self.meta.sums = sums;
        self.inflight.push((seq, lease));
        self.meta.calls_today = self.meta.calls_today.saturating_add(1);
        self.save_meta(store)?;
        Ok(LedgerReply::Admitted { seq })
    }

    /// Settlement (design 3.7), idempotent: a row no longer in flight answers `done`.
    pub fn finish(
        &mut self,
        store: &mut impl Store,
        cfg: &LedgerConfig,
        now: i64,
        op: &FinishOp,
    ) -> Result<LedgerReply, LedgerError> {
        let state = match op.outcome.as_str() {
            "answered" => STATE_ANSWERED,
            "provider_unavailable" => STATE_PROVIDER_UNAVAILABLE,
            "timeout" => STATE_TIMEOUT,
            _ => return Ok(LedgerReply::error("invalid_op")),
        };
        if op.nusd.is_some_and(|n| !(0..=MAX_NUSD).contains(&n)) {
            return Ok(LedgerReply::error("invalid_op"));
        }
        let rows = store.exec(sql::QUESTION_HEAD, &[op.seq.into()])?;
        let Some(row) = rows.first() else {
            return Ok(LedgerReply::error("unknown_reservation"));
        };
        let (Some(at), Some(old), Some(current)) =
            (col_i64(row, 0), col_i64(row, 1), col_i64(row, 2))
        else {
            return Ok(LedgerReply::error("corrupt_row"));
        };
        if current != STATE_INFLIGHT {
            return Ok(LedgerReply::Done);
        }
        let nusd = op.nusd.unwrap_or(old);
        if self.valid && nusd != old {
            let delta = nusd
                .checked_sub(old)
                .ok_or(LedgerError::Abort("overflow"))?;
            self.shift_sums(at, delta)?;
        }
        let answered = state == STATE_ANSWERED;
        let answer = match (answered, op.answer.as_deref()) {
            (true, Some(answer)) if self.keep_text(cfg, answer) => Some(answer),
            _ => None,
        };
        let only_answered = |v: SqlValue| if answered { v } else { SqlValue::Null };
        store.exec(
            sql::QUESTION_FINISH,
            &[
                state.into(),
                nusd.into(),
                now.into(),
                only_answered(op.answer_id.clone().into()),
                answer.into(),
                only_answered(op.answer_len.into()),
                only_answered(op.model.clone().into()),
                op.usage.clone().into(),
                op.latency_ms.into(),
                if answered {
                    SqlValue::Null
                } else {
                    op.error.clone().into()
                },
                op.seq.into(),
            ],
        )?;
        self.inflight.retain(|(seq, _)| *seq != op.seq);
        self.save_meta(store)?;
        Ok(LedgerReply::Done)
    }

    fn load_cmd_window(&mut self, store: &mut impl Store, now: i64) -> Result<(), LedgerError> {
        if self.cmd_window.is_some() {
            return Ok(());
        }
        let mut window = VecDeque::new();
        if self.meta.cmd_last_at > now - COMMAND_WINDOW_SECS {
            let limit = i64::try_from(COMMAND_WINDOW_MAX).unwrap_or(120);
            let rows = store.exec(sql::COMMAND_RECENT_AT, &[limit.into()])?;
            window.extend(rows.iter().rev().filter_map(|row| col_i64(row, 0)));
        }
        self.cmd_window = Some(window);
        Ok(())
    }

    /// One command line (design 3.8): daily cap, then the global 120 / 60 s window.
    pub fn command(
        &mut self,
        store: &mut impl Store,
        cfg: &LedgerConfig,
        now: i64,
        op: &CommandOp,
    ) -> Result<LedgerReply, LedgerError> {
        if op.mode != "classic" && op.mode != "ai" {
            return Ok(LedgerReply::error("invalid_op"));
        }
        self.load_cmd_window(store, now)?;
        let day = day_number(now);
        if self.meta.cmd_day != day {
            self.meta.cmd_day = day;
            self.meta.cmd_today = 0;
        }
        if self.meta.cmd_today >= cfg.command_daily_cap {
            return Ok(LedgerReply::Refused {
                reason: "daily_cap".into(),
            });
        }
        let full = match self.cmd_window.as_mut() {
            Some(window) => {
                window.retain(|t| now.saturating_sub(*t) < COMMAND_WINDOW_SECS);
                window.len() >= COMMAND_WINDOW_MAX
            }
            None => false,
        };
        if full {
            return Ok(LedgerReply::Refused {
                reason: "window_full".into(),
            });
        }
        let text = self
            .keep_text(cfg, &op.command)
            .then_some(op.command.as_str());
        let seq = self.meta.cmd_next_seq;
        store.exec(
            sql::COMMAND_INSERT,
            &[
                seq.into(),
                now.into(),
                text.into(),
                op.command_len.into(),
                op.mode.as_str().into(),
                op.ip.as_str().into(),
            ],
        )?;
        self.meta.cmd_next_seq = seq.saturating_add(1);
        self.meta.cmd_today = self.meta.cmd_today.saturating_add(1);
        self.meta.cmd_last_at = now;
        self.save_meta(store)?;
        if let Some(window) = self.cmd_window.as_mut() {
            window.push_back(now);
        }
        Ok(LedgerReply::Done)
    }

    /// Health probe: validity, text budget, database size. Writes nothing.
    pub fn ping(
        &mut self,
        store: &mut impl Store,
        cfg: &LedgerConfig,
    ) -> Result<LedgerReply, LedgerError> {
        Ok(LedgerReply::Pong {
            ledger_ok: self.valid && cfg.limits.is_some(),
            text_full: self.text_full(cfg),
            db_bytes: store.database_size()?,
        })
    }

    /// Id-keyed, idempotent import of VPS ledger charges (design 3.10).
    pub fn import(
        &mut self,
        store: &mut impl Store,
        now: i64,
        op: &ImportOp,
    ) -> Result<LedgerReply, LedgerError> {
        if self.meta.import_closed != 0 {
            return Ok(LedgerReply::error("import_closed"));
        }
        if !self.valid {
            return Ok(LedgerReply::error("invalid_book"));
        }
        if op.charges.len() > IMPORT_MAX {
            return Ok(LedgerReply::error("too_many_charges"));
        }
        let mut charges = Vec::with_capacity(op.charges.len());
        for (index, value) in op.charges.iter().enumerate() {
            match parse_charge(value) {
                Some(charge) => charges.push(charge),
                None => return Ok(LedgerReply::error(format!("invalid_charge:{index}"))),
            }
        }
        self.advance(store, now)?;
        if !self.valid {
            return Ok(LedgerReply::error("invalid_book"));
        }
        let (mut inserted, mut updated, mut skipped) = (0, 0, 0);
        for charge in charges {
            let at = charge.at.min(now);
            if at <= now - MONTH {
                skipped += 1;
                continue;
            }
            let id = charge.id.as_str();
            let existing = match query_i64(store, sql::IMPORTED_SEQ, &[id.into()])? {
                Some(seq) => store
                    .exec(sql::QUESTION_HEAD, &[seq.into()])?
                    .first()
                    .and_then(|row| Some((seq, col_i64(row, 0)?, col_i64(row, 1)?))),
                None => None,
            };
            match existing {
                Some((seq, row_at, old)) => {
                    if charge.nusd != old {
                        store.exec(sql::QUESTION_SET_NUSD, &[charge.nusd.into(), seq.into()])?;
                        let delta = charge
                            .nusd
                            .checked_sub(old)
                            .ok_or(LedgerError::Abort("overflow"))?;
                        self.shift_sums(row_at, delta)?;
                    }
                    updated += 1;
                }
                None => {
                    let seq = self.meta.next_seq;
                    store.exec(
                        sql::IMPORTED_QUESTION_INSERT,
                        &[seq.into(), id.into(), at.into(), charge.nusd.into()],
                    )?;
                    store.exec(sql::IMPORTED_UPSERT, &[id.into(), seq.into()])?;
                    self.meta.next_seq = seq.saturating_add(1);
                    self.shift_sums(at, charge.nusd)?;
                    inserted += 1;
                }
            }
        }
        if !self.valid {
            return Err(LedgerError::Abort("negative_sum"));
        }
        self.save_meta(store)?;
        Ok(LedgerReply::Imported {
            inserted,
            updated,
            skipped,
        })
    }

    /// Ends the VPS import for good.
    pub fn import_close(&mut self, store: &mut impl Store) -> Result<LedgerReply, LedgerError> {
        if !self.valid {
            return Ok(LedgerReply::error("invalid_book"));
        }
        self.meta.import_closed = 1;
        self.save_meta(store)?;
        Ok(LedgerReply::Done)
    }

    /// One NDJSON page (design 3.12), bounded by rows and by 256 KiB.
    pub fn export(
        &mut self,
        store: &mut impl Store,
        now: i64,
        op: &ExportOp,
    ) -> Result<ExportOutcome, LedgerError> {
        let Some(table) = Table::parse(&op.table) else {
            return Ok(ExportOutcome::Rejected("invalid_table"));
        };
        let Some(cursor) = parse_cursor(table, &op.after) else {
            return Ok(ExportOutcome::Rejected("invalid_cursor"));
        };
        let limit = table.page_rows(op.limit);
        let after_seq = match cursor {
            Cursor::Seq(seq) => seq,
            _ => 0,
        };
        let mut body = Vec::new();
        let mut last: Option<Cursor> = None;
        let rows = match table {
            Table::Questions => {
                store.exec(sql::EXPORT_QUESTIONS, &[after_seq.into(), limit.into()])?
            }
            Table::Commands => {
                store.exec(sql::EXPORT_COMMANDS, &[after_seq.into(), limit.into()])?
            }
            Table::Ledger => {
                let edge = now - MONTH;
                let (at, seq) = match cursor {
                    Cursor::AtSeq(at, seq) => (at, seq),
                    _ => (edge, 0),
                };
                store.exec(
                    sql::EXPORT_LEDGER,
                    &[at.into(), edge.into(), at.into(), seq.into(), limit.into()],
                )?
            }
        };
        let fetched = i64::try_from(rows.len()).unwrap_or(i64::MAX);
        let mut stopped = false;
        for row in &rows {
            let (lines, position) = match table {
                Table::Questions => {
                    let q = decode_question(row).ok_or(LedgerError::Abort("corrupt_row"))?;
                    if !exportable(&q, now) {
                        stopped = true;
                        break;
                    }
                    (question_lines(&q), Cursor::Seq(q.seq))
                }
                Table::Commands => {
                    let (Some(seq), Some(at), Some(len)) =
                        (col_i64(row, 0), col_i64(row, 1), col_i64(row, 3))
                    else {
                        return Err(LedgerError::Abort("corrupt_row"));
                    };
                    let command = col_text(row, 2);
                    let mode = col_text(row, 4).unwrap_or_default();
                    let ip = col_text(row, 5).unwrap_or_default();
                    (
                        command_line(at, command.as_deref(), len, &mode, &ip),
                        Cursor::Seq(seq),
                    )
                }
                Table::Ledger => {
                    let (Some(seq), Some(id), Some(at), Some(nusd)) = (
                        col_i64(row, 0),
                        col_text(row, 1),
                        col_i64(row, 2),
                        col_i64(row, 3),
                    ) else {
                        return Err(LedgerError::Abort("corrupt_row"));
                    };
                    (ledger_line(&id, at, nusd), Cursor::AtSeq(at, seq))
                }
            };
            if !body.is_empty() && body.len() + lines.len() > PAGE_BYTES {
                stopped = true;
                break;
            }
            body.extend_from_slice(&lines);
            last = Some(position);
        }
        let more = stopped || fetched >= limit;
        Ok(ExportOutcome::Page {
            body,
            next_after: if more { last.map(format_cursor) } else { None },
        })
    }

    /// Deletes ≤ 1,000 rows older than `before` (design 3.11); questions never inside the
    /// 30-day window.
    pub fn purge(
        &mut self,
        store: &mut impl Store,
        now: i64,
        op: &PurgeOp,
    ) -> Result<LedgerReply, LedgerError> {
        if !self.valid {
            return Ok(LedgerReply::error("invalid_book"));
        }
        let (deleted, bytes) = match op.table.as_str() {
            "questions" => {
                self.advance(store, now)?;
                if !self.valid {
                    return Ok(LedgerReply::error("invalid_book"));
                }
                let cutoff = op.before.min(now - MONTH);
                delete_rows(
                    store,
                    sql::QUESTIONS_DELETE_UPTO,
                    &[cutoff.into(), PURGE_BATCH.into()],
                )?
            }
            "commands" => {
                let cutoff = op.before.min(now);
                delete_rows(
                    store,
                    sql::COMMANDS_DELETE_UPTO,
                    &[PURGE_BATCH.into(), cutoff.into()],
                )?
            }
            _ => return Ok(LedgerReply::error("invalid_table")),
        };
        self.meta.text_bytes = self.meta.text_bytes.saturating_sub(bytes).max(0);
        self.save_meta(store)?;
        Ok(LedgerReply::Purged {
            deleted,
            more: deleted >= PURGE_BATCH,
        })
    }

    /// Operator view (design 3.12): no unbounded `COUNT(*)` over questions or commands.
    pub fn stats(
        &mut self,
        store: &mut impl Store,
        cfg: &LedgerConfig,
        now: i64,
    ) -> Result<LedgerReply, LedgerError> {
        if self.valid {
            self.advance(store, now)?;
        }
        let span = |max: Option<i64>, min: Option<i64>| match (max, min) {
            (Some(max), Some(min)) => max - min + 1,
            _ => 0,
        };
        let questions_rows = span(
            query_i64(store, sql::QUESTIONS_MAX_SEQ, &[])?,
            query_i64(store, sql::QUESTIONS_MIN_SEQ, &[])?,
        );
        let commands_rows = span(
            query_i64(store, sql::COMMANDS_MAX_SEQ, &[])?,
            query_i64(store, sql::COMMANDS_MIN_SEQ, &[])?,
        );
        let imported_rows = query_count(store, sql::IMPORTED_COUNT, &[])?;
        let unknown_ip_questions_24h =
            query_count(store, sql::UNKNOWN_IP_QUESTIONS, &[(now - DAY).into()])?;
        let unknown_ip_commands_last_10k = query_count(
            store,
            sql::UNKNOWN_IP_COMMANDS,
            &[self
                .meta
                .cmd_next_seq
                .saturating_sub(UNKNOWN_IP_COMMAND_SCAN + 1)
                .into()],
        )?;
        let day = day_number(now);
        let meta = &self.meta;
        Ok(LedgerReply::Stats(Box::new(StatsReply {
            schema_version: meta.schema_version,
            min_reader_version: meta.min_reader_version,
            reader_version: READER_VERSION,
            valid: self.valid,
            invalid_reason: self.invalid_reason.map(str::to_string),
            budget_valid: cfg.limits.is_some(),
            sums_usd: WindowsUsd::from_values(meta.sums.map(usd)),
            limits_usd: WindowsUsd::from_values(cfg.limits_usd),
            inflight: self
                .inflight
                .iter()
                .filter(|(_, lease)| *lease > now)
                .map(|&(seq, lease_until)| Lease { seq, lease_until })
                .collect(),
            calls_today: if meta.calls_day == day {
                meta.calls_today
            } else {
                0
            },
            daily_max_calls: cfg.daily_max_calls,
            cmd_today: if meta.cmd_day == day {
                meta.cmd_today
            } else {
                0
            },
            command_daily_cap: cfg.command_daily_cap,
            ip_count: meta.ip_count,
            next_seq: meta.next_seq,
            cmd_next_seq: meta.cmd_next_seq,
            questions_rows,
            commands_rows,
            db_bytes: store.database_size()?,
            text_bytes: meta.text_bytes,
            text_budget_bytes: cfg.text_budget_bytes,
            text_dropped: meta.text_dropped,
            retention_days: cfg.retention_days,
            maintained_at: meta.maintained_at,
            import_closed: meta.import_closed != 0,
            imported_rows,
            unknown_ip_questions_24h,
            unknown_ip_commands_last_10k,
        })))
    }

    /// Manual repair (design 3.11): recomputes `meta` from the rows, bounded by the 30-day window
    /// and the covering index, and writes it even when the book was invalid.
    pub fn rebuild(
        &mut self,
        store: &mut impl Store,
        now: i64,
    ) -> Result<LedgerReply, LedgerError> {
        let mut meta = self.meta.clone();
        meta.schema_version = SCHEMA_VERSION;
        meta.min_reader_version = READER_VERSION;
        for ((edge, sum), window) in meta.edges.iter_mut().zip(meta.sums.iter_mut()).zip(WINDOWS) {
            *edge = now - window;
            *sum = query_count(store, sql::SUM_AFTER, &[(*edge).into()])?;
        }
        let mut inflight: Vec<(i64, i64)> = store
            .exec(
                sql::INFLIGHT_ROWS,
                &[(now - COMMAND_WINDOW_SECS).into(), now.into()],
            )?
            .iter()
            .filter_map(|row| Some((col_i64(row, 0)?, col_i64(row, 1)?)))
            .collect();
        inflight.sort_unstable();
        meta.next_seq =
            query_i64(store, sql::QUESTIONS_MAX_SEQ, &[])?.map_or(1, |m| m.saturating_add(1));
        meta.cmd_next_seq =
            query_i64(store, sql::COMMANDS_MAX_SEQ, &[])?.map_or(1, |m| m.saturating_add(1));
        let day = day_number(now);
        let day_start = day * DAY;
        meta.calls_day = day;
        meta.calls_today = query_count(store, sql::CALLS_SINCE, &[day_start.into()])?;
        meta.cmd_day = day;
        meta.cmd_today = query_count(
            store,
            sql::COMMANDS_SINCE,
            &[
                meta.cmd_next_seq.saturating_sub(CMD_TODAY_SCAN + 1).into(),
                day_start.into(),
            ],
        )?;
        meta.cmd_last_at = newest_command_at(store)?.unwrap_or(0);
        meta.ip_count = query_count(store, sql::IP_COUNT, &[])?;
        meta.text_bytes = store.database_size()?;
        meta.inflight = format_inflight(&inflight);
        let problem = meta.problem();
        store.exec(sql::META_UPSERT, &meta.to_params())?;
        let events = std::mem::take(&mut self.events);
        *self = Book {
            meta,
            inflight,
            valid: problem.is_none(),
            invalid_reason: problem,
            cmd_window: None,
            events,
        };
        Ok(match problem {
            None => LedgerReply::Done,
            Some(reason) => LedgerReply::error(format!("rebuild_invalid:{reason}")),
        })
    }

    /// Lazy maintenance (design 2.8): at most once per hour, bounded batches.
    pub fn maintain(
        &mut self,
        store: &mut impl Store,
        cfg: &LedgerConfig,
        now: i64,
    ) -> Result<Option<Maintenance>, LedgerError> {
        if !self.valid || now.saturating_sub(self.meta.maintained_at) < MAINTENANCE_EVERY {
            return Ok(None);
        }
        // Advance first: no row deleted below may still be counted in a window sum.
        self.advance(store, now)?;
        if !self.valid {
            return Ok(None);
        }
        let ips = store
            .exec(sql::IP_PRUNE, &[(now - IP_HISTORY).into()])?
            .len();
        let ips = i64::try_from(ips).unwrap_or(i64::MAX);
        self.meta.ip_count = self.meta.ip_count.saturating_sub(ips).max(0);
        self.meta.ips_pruned_at = now;
        let mut report = Maintenance {
            ips,
            ..Maintenance::default()
        };
        if cfg.retention_days > 0 {
            let cutoff = now - cfg.retention_days * DAY;
            let (questions, question_bytes) = delete_rows(
                store,
                sql::QUESTIONS_DELETE_UPTO,
                &[cutoff.into(), MAINTENANCE_BATCH.into()],
            )?;
            let (commands, command_bytes) = delete_rows(
                store,
                sql::COMMANDS_DELETE_UPTO,
                &[MAINTENANCE_BATCH.into(), cutoff.into()],
            )?;
            report.questions = questions;
            report.commands = commands;
            report.text_bytes_freed = question_bytes + command_bytes;
            report.backlog = questions >= MAINTENANCE_BATCH || commands >= MAINTENANCE_BATCH;
            self.meta.text_bytes = self
                .meta
                .text_bytes
                .saturating_sub(report.text_bytes_freed)
                .max(0);
        }
        self.meta.maintained_at = if report.backlog {
            now - (MAINTENANCE_EVERY - MAINTENANCE_BACKLOG_DELAY)
        } else {
            now
        };
        self.save_meta(store)?;
        Ok(Some(report))
    }
}

fn newest_command_at(store: &mut impl Store) -> Result<Option<i64>, LedgerError> {
    Ok(query_i64(store, sql::COMMAND_RECENT_AT, &[1i64.into()])?)
}

/// Runs a `DELETE … RETURNING <text bytes>`: (rows deleted, text bytes freed).
fn delete_rows(
    store: &mut impl Store,
    statement: &'static str,
    params: &[SqlValue],
) -> Result<(i64, i64), LedgerError> {
    let rows = store.exec(statement, params)?;
    let bytes = rows.iter().filter_map(|row| col_i64(row, 0)).sum();
    Ok((i64::try_from(rows.len()).unwrap_or(i64::MAX), bytes))
}

/// A VPS charge `{"id","at","usd"}`: id non-empty and ≤ 64 characters, `at` an integer, `usd`
/// finite, ≥ 0 and ≤ 1e6.
fn parse_charge(value: &Value) -> Option<Charge> {
    let id = value.get("id")?.as_str()?;
    if id.is_empty() || id.chars().count() > MAX_CHARGE_ID_CHARS {
        return None;
    }
    let at = value.get("at")?.as_i64()?;
    let usd = value.get("usd")?.as_f64()?;
    if !usd.is_finite() || !(0.0..=1e6).contains(&usd) {
        return None;
    }
    Some(Charge {
        id: id.to_string(),
        at,
        nusd: (usd * 1e9).round() as i64,
    })
}

fn decode_question(row: &[SqlValue]) -> Option<QuestionRow> {
    Some(QuestionRow {
        seq: col_i64(row, 0)?,
        qid: col_text(row, 1)?,
        at: col_i64(row, 2)?,
        nusd: col_i64(row, 3)?,
        state: col_i64(row, 4)?,
        lease_until: col_i64(row, 5)?,
        ip: col_text(row, 6),
        question: col_text(row, 7),
        question_len: col_i64(row, 8),
        answer_id: col_text(row, 9),
        answer: col_text(row, 10),
        answer_len: col_i64(row, 11),
        model: col_text(row, 12),
        finished_at: col_i64(row, 13),
        usage: col_text(row, 14),
        latency_ms: col_i64(row, 15),
        error: col_text(row, 16),
    })
}

#[cfg(test)]
mod tests;
