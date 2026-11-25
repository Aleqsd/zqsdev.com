//! Native tests of the ledger on real SQLite: every op goes through `serve` (JSON in, JSON out),
//! as the Worker reaches the Durable Object.

use std::cell::RefCell;

use serde_json::{json, Value};

use super::meta::{Meta, MONTH};
use super::protocol::{
    AdmitOp, CommandOp, ExportOp, FinishOp, ImportOp, LedgerOp, LedgerReply, PurgeOp, StatsReply,
};
use super::serve::{serve, DoLog, DoResponse};
use super::sql;
use super::sqlite_store::SqliteHost;
use super::store::SqlValue;
use crate::core::config::LedgerConfig;
use crate::core::logline::{Level, Throttle};
use crate::core::testkit::ledger_config;

/// 2026-11-24T00:00:00Z.
const DAY0: i64 = 1_795_478_400;
const USD: i64 = 1_000_000_000;

struct Ledger {
    host: SqliteHost,
    cfg: LedgerConfig,
    throttle: Throttle,
    logs: RefCell<Vec<(Level, &'static str, Value)>>,
}

impl Ledger {
    fn new(pairs: &[(&str, &str)]) -> Ledger {
        Ledger {
            host: SqliteHost::new(),
            cfg: ledger_config(pairs),
            throttle: Throttle::new(),
            logs: RefCell::new(Vec::new()),
        }
    }

    /// Generous spend limits: only the rule under test refuses.
    fn generous() -> Ledger {
        Ledger::new(&[
            ("AI_MINUTE_BUDGET_USD", "1000"),
            ("AI_HOUR_BUDGET_USD", "1000"),
            ("AI_DAY_BUDGET_USD", "1000"),
            ("AI_MONTH_BUDGET_USD", "1000"),
            ("AI_DAILY_MAX_CALLS", "100000"),
        ])
    }

    fn raw(&self, now: i64, body: &[u8]) -> DoResponse {
        let sink = |level: Level, msg: &'static str, fields: Value| {
            self.logs.borrow_mut().push((level, msg, fields));
        };
        let log = DoLog {
            sink: &sink,
            throttle: &self.throttle,
            now_ms: now as u64 * 1000,
        };
        serve(&self.host, self.cfg, now as u64 * 1000 + 999, body, &log)
    }

    fn op(&self, now: i64, op: LedgerOp) -> LedgerReply {
        let response = self.raw(now, &serde_json::to_vec(&op).unwrap());
        assert_eq!(response.content_type, "application/json");
        serde_json::from_slice(&response.body).unwrap()
    }

    fn admit(&self, now: i64, ip: &str, nusd: i64) -> LedgerReply {
        self.op(
            now,
            LedgerOp::Admit(AdmitOp {
                qid: format!("q-{now}-{ip}-{nusd}"),
                ip: ip.into(),
                ip_key: ip.into(),
                nusd,
                question: "Micro Mages?".into(),
                question_len: 12,
            }),
        )
    }

    fn admitted(&self, now: i64, ip: &str, nusd: i64) -> i64 {
        match self.admit(now, ip, nusd) {
            LedgerReply::Admitted { seq } => seq,
            other => panic!("not admitted at {now}: {other:?}"),
        }
    }

    fn finish(&self, now: i64, seq: i64, outcome: &str, nusd: Option<i64>) -> LedgerReply {
        self.op(
            now,
            LedgerOp::Finish(FinishOp {
                seq,
                outcome: outcome.into(),
                nusd,
                answer_id: Some("a".into()),
                answer: Some("Python.".into()),
                answer_len: Some(7),
                model: Some("gpt-6-luna".into()),
                usage: Some(r#"{"prompt_tokens":1}"#.into()),
                latency_ms: Some(5),
                error: (outcome != "answered").then(|| "http_429".to_string()),
            }),
        )
    }

    fn command(&self, now: i64, text: &str) -> LedgerReply {
        self.op(
            now,
            LedgerOp::Command(CommandOp {
                command: text.into(),
                command_len: text.chars().count() as i64,
                mode: "classic".into(),
                ip: "203.0.113.7".into(),
            }),
        )
    }

    fn import(&self, now: i64, charges: Value) -> LedgerReply {
        self.op(
            now,
            LedgerOp::Import(ImportOp {
                charges: charges.as_array().unwrap().clone(),
            }),
        )
    }

    fn stats(&self, now: i64) -> StatsReply {
        match self.op(now, LedgerOp::Stats) {
            LedgerReply::Stats(stats) => *stats,
            other => panic!("{other:?}"),
        }
    }

    /// Window sums (nano-USD) at `now`, from the cached book after `stats` advanced it.
    fn sums(&self, now: i64) -> [i64; 4] {
        self.stats(now);
        self.host.book.borrow().as_ref().unwrap().meta().sums
    }

    /// The stored meta row.
    fn meta(&self) -> Meta {
        let rows = self.host.store.borrow_mut().rows(sql::META_SELECT);
        Meta::from_row(&rows[0]).unwrap()
    }

    fn count(&self, sql: &str) -> i64 {
        self.host.store.borrow_mut().count(sql)
    }

    fn exec(&self, sql: &str) {
        self.host.store.borrow().conn.execute_batch(sql).unwrap();
    }

    /// Every row of every table, to prove that an op wrote nothing.
    fn snapshot(&self) -> Vec<Vec<Vec<SqlValue>>> {
        let mut store = self.host.store.borrow_mut();
        [
            "meta",
            "questions",
            "ip_windows",
            "commands",
            "imported",
            "delete_budget",
        ]
        .iter()
        .map(|t| store.rows(&format!("SELECT * FROM {t}")))
        .collect()
    }

    fn executed(&self) -> Vec<&'static str> {
        std::mem::take(&mut self.host.store.borrow_mut().executed)
    }

    fn export(&self, now: i64, table: &str, after: &str, limit: Option<i64>) -> DoResponse {
        let op = LedgerOp::Export(ExportOp {
            table: table.into(),
            after: after.into(),
            limit,
        });
        self.raw(now, &serde_json::to_vec(&op).unwrap())
    }

    fn logged(&self, msg: &str) -> usize {
        self.logs
            .borrow()
            .iter()
            .filter(|(_, m, _)| *m == msg)
            .count()
    }
}

fn reason(reply: &LedgerReply) -> (String, String) {
    match reply {
        LedgerReply::Denied { reason, detail } => {
            (reason.clone(), detail.clone().unwrap_or_default())
        }
        other => panic!("not denied: {other:?}"),
    }
}

// ---------------------------------------------------------------- ported from budget.rs

#[test]
fn settlement_replaces_reservation_and_survives_restart() {
    let l = Ledger::new(&[]);
    let seq = l.admitted(DAY0, "198.51.100.1", USD / 10);
    assert_eq!(
        l.finish(DAY0, seq, "answered", Some(USD / 50)),
        LedgerReply::Done
    );
    l.host.evict();
    assert_eq!(l.count("SELECT COUNT(*) FROM questions"), 1);
    assert_eq!(l.count("SELECT nusd FROM questions"), 20_000_000);
    assert_eq!(l.meta().sums[3], 20_000_000);
    // The reopened ledger counts the settled amount.
    assert_eq!(l.stats(DAY0 + 1).sums_usd.month, Some(0.02));
}

#[test]
fn crash_retains_reservation_and_blocks_overspend() {
    let l = Ledger::new(&[]);
    l.admitted(100, "198.51.100.1", 490_000_000);
    l.host.evict();
    let denied = l.admit(101, "198.51.100.2", 20_000_000);
    assert_eq!(reason(&denied), ("spending_limit".into(), "minute".into()));
    l.admitted(161, "198.51.100.3", 20_000_000);
}

#[test]
fn ip_limits_expire() {
    let l = Ledger::generous();
    for _ in 0..4 {
        let seq = l.admitted(100, "a", 1_000);
        l.finish(100, seq, "answered", Some(500));
    }
    let denied = l.admit(100, "a", 1_000);
    assert_eq!(
        reason(&denied),
        ("request_limit".into(), "ip_window_1s".into())
    );
    let seq = l.admitted(102, "a", 1_000);
    l.finish(102, seq, "answered", Some(500));
    // An admission a day later triggers the due maintenance, which prunes the stale key.
    l.admitted(100_000, "b", 1_000);
    let keys = l
        .host
        .store
        .borrow_mut()
        .rows("SELECT ip_key FROM ip_windows");
    assert_eq!(keys, vec![vec![SqlValue::Text("b".into())]]);
    assert_eq!(l.meta().ip_count, 1);
    assert_eq!(l.logged("🧹 Ledger maintenance"), 1);
}

#[test]
fn corrupted_ledger_fails_closed() {
    let l = Ledger::generous();
    // An invalid import chunk writes nothing.
    let reply = l.import(
        DAY0,
        json!([{"id":"ok","at":DAY0,"usd":0.1},{"id":"bad","at":DAY0,"usd":-1}]),
    );
    assert_eq!(reply, LedgerReply::error("invalid_charge:1"));
    assert_eq!(l.count("SELECT COUNT(*) FROM questions"), 0);
    assert_eq!(l.count("SELECT COUNT(*) FROM imported"), 0);
    for bad in [
        json!([{"id":"","at":DAY0,"usd":0.1}]),
        json!([{"id":"x".repeat(65),"at":DAY0,"usd":0.1}]),
        json!([{"id":"x","at":1.5,"usd":0.1}]),
        json!([{"id":"x","at":DAY0,"usd":1_000_001.0}]),
        json!([{"id":"x","at":DAY0}]),
        json!([{"id":7,"at":DAY0,"usd":0.1}]),
        json!(["x"]),
    ] {
        assert_eq!(l.import(DAY0, bad), LedgerReply::error("invalid_charge:0"));
    }

    // An invalid meta row fails closed: admissions denied, ping not ok, commands still work.
    l.admitted(DAY0, "a", 1_000);
    for corruption in [
        "UPDATE meta SET min_reader_version = 2",
        "UPDATE meta SET sum_hour = -5",
        "UPDATE meta SET inflight = 'garbage'",
        "UPDATE meta SET next_seq = 0",
        "DELETE FROM meta",
    ] {
        let saved = l.snapshot();
        l.exec(corruption);
        l.host.evict();
        let corrupted_meta = l.host.store.borrow_mut().rows(sql::META_SELECT);
        let denied = l.admit(DAY0 + 10, "c", 1_000);
        assert_eq!(reason(&denied).0, "budget_unavailable", "{corruption}");
        match l.op(DAY0 + 10, LedgerOp::Ping) {
            LedgerReply::Pong { ledger_ok, .. } => assert!(!ledger_ok, "{corruption}"),
            other => panic!("{other:?}"),
        }
        let commands_before = l.count("SELECT COUNT(*) FROM commands");
        assert_eq!(
            l.command(DAY0 + 10, "help"),
            LedgerReply::Done,
            "{corruption}"
        );
        assert_eq!(
            l.count("SELECT COUNT(*) FROM commands"),
            commands_before + 1
        );
        // The untrusted meta row is never written (nor re-created).
        let meta_after = l.host.store.borrow_mut().rows(sql::META_SELECT);
        assert_eq!(meta_after, corrupted_meta, "{corruption}");
        assert!(!l.stats(DAY0 + 10).valid);
        // Restore for the next corruption.
        l.exec("DELETE FROM meta; DELETE FROM commands;");
        let meta_rows = &saved[0];
        let params: Vec<String> = meta_rows[0]
            .iter()
            .map(|v| match v {
                SqlValue::Int(i) => i.to_string(),
                SqlValue::Text(t) => format!("'{t}'"),
                other => panic!("{other:?}"),
            })
            .collect();
        l.exec(&format!("INSERT INTO meta VALUES ({})", params.join(",")));
        l.host.evict();
    }
    // Rebuild repairs an invalid book.
    l.exec("UPDATE meta SET sum_month = -1");
    l.host.evict();
    assert_eq!(
        reason(&l.admit(DAY0 + 20, "c", 1_000)).0,
        "budget_unavailable"
    );
    assert_eq!(l.op(DAY0 + 20, LedgerOp::Rebuild), LedgerReply::Done);
    l.admitted(DAY0 + 21, "c", 1_000);
}

// ---------------------------------------------------------------- windows

#[test]
fn window_boundaries_are_exact() {
    let l = Ledger::new(&[]);
    let a = l.admitted(1_000, "a", 300_000_000);
    l.finish(1_000, a, "provider_unavailable", None);
    // sum + nusd == limit is admitted (strict comparison).
    let b = l.admitted(1_059, "b", 200_000_000);
    l.finish(1_059, b, "provider_unavailable", None);
    assert_eq!(
        reason(&l.admit(1_059, "c", 1)),
        ("spending_limit".into(), "minute".into())
    );
    // now − at == 60: the first charge left the minute window.
    l.admitted(1_060, "c", 300_000_000);
    let s = l.stats(1_060);
    assert_eq!(s.sums_usd.minute, Some(0.5));
    assert_eq!(s.sums_usd.hour, Some(0.8));
}

#[test]
fn month_expiry_after_a_30_day_jump() {
    let l = Ledger::new(&[
        ("AI_MINUTE_BUDGET_USD", "100"),
        ("AI_HOUR_BUDGET_USD", "100"),
        ("AI_DAY_BUDGET_USD", "100"),
        ("AI_MONTH_BUDGET_USD", "10"),
    ]);
    let t = DAY0;
    let seq = l.admitted(t, "a", 9_500_000_000);
    l.finish(t, seq, "timeout", None);
    assert_eq!(
        reason(&l.admit(t + 1, "b", USD)),
        ("spending_limit".into(), "month".into())
    );
    assert_eq!(
        reason(&l.admit(t + MONTH - 1, "b", USD)),
        ("spending_limit".into(), "month".into())
    );
    l.admitted(t + MONTH, "b", USD);
    let s = l.stats(t + MONTH);
    assert_eq!(s.sums_usd.month, Some(1.0));
    assert_eq!(s.sums_usd.minute, Some(1.0));
}

#[test]
fn finish_after_the_row_left_the_minute_window() {
    let l = Ledger::generous();
    let first = l.admitted(1_000, "a", 400_000_000);
    // A later admission moves the minute edge past the first row.
    l.admitted(1_070, "b", 100_000_000);
    assert_eq!(
        l.finish(1_070, first, "answered", Some(100_000_000)),
        LedgerReply::Done
    );
    let s = l.stats(1_070);
    assert_eq!(s.sums_usd.minute, Some(0.1));
    assert_eq!(s.sums_usd.hour, Some(0.2));
    assert_eq!(s.sums_usd.month, Some(0.2));
    // The incremental sums equal a full recomputation.
    let before = l.meta().sums;
    assert_eq!(l.op(1_070, LedgerOp::Rebuild), LedgerReply::Done);
    assert_eq!(l.meta().sums, before);
}

#[test]
fn finish_is_idempotent_and_validated() {
    let l = Ledger::generous();
    let seq = l.admitted(DAY0, "a", 2_532_000);
    assert_eq!(
        l.finish(DAY0 + 1, seq, "answered", Some(20_000)),
        LedgerReply::Done
    );
    let after_first = l.snapshot();
    assert_eq!(
        l.finish(DAY0 + 2, seq, "answered", Some(99_999)),
        LedgerReply::Done
    );
    assert_eq!(l.finish(DAY0 + 2, seq, "timeout", None), LedgerReply::Done);
    assert_eq!(l.snapshot(), after_first);
    assert_eq!(
        l.finish(DAY0, 999, "answered", Some(1)),
        LedgerReply::error("unknown_reservation")
    );
    assert_eq!(
        l.finish(DAY0, seq, "lost", None),
        LedgerReply::error("invalid_op")
    );
    assert_eq!(
        l.finish(DAY0, seq, "answered", Some(-1)),
        LedgerReply::error("invalid_op")
    );
    let row =
        l.host.store.borrow_mut().rows(
            "SELECT state, nusd, lease_until, finished_at, answer, model, error FROM questions",
        );
    assert_eq!(
        row[0],
        vec![
            SqlValue::Int(1),
            SqlValue::Int(20_000),
            SqlValue::Int(0),
            SqlValue::Int(DAY0 + 1),
            SqlValue::Text("Python.".into()),
            SqlValue::Text("gpt-6-luna".into()),
            SqlValue::Null,
        ]
    );
    assert!(l.stats(DAY0 + 2).inflight.is_empty());
}

// ---------------------------------------------------------------- slots and caps

#[test]
fn lease_expiry_frees_a_slot() {
    let l = Ledger::generous();
    l.admitted(100, "a", 1_000);
    l.admitted(100, "b", 1_000);
    assert_eq!(
        reason(&l.admit(100, "c", 1_000)),
        ("busy".into(), "slots".into())
    );
    assert_eq!(reason(&l.admit(119, "c", 1_000)).0, "busy");
    l.admitted(120, "c", 1_000);
    let s = l.stats(120);
    assert_eq!(s.inflight.len(), 1);
}

#[test]
fn daily_call_cap_resets_at_utc_midnight() {
    let mut pairs = vec![("AI_DAILY_MAX_CALLS", "2")];
    pairs.extend([
        ("AI_MINUTE_BUDGET_USD", "1000"),
        ("AI_HOUR_BUDGET_USD", "1000"),
        ("AI_DAY_BUDGET_USD", "1000"),
        ("AI_MONTH_BUDGET_USD", "1000"),
    ]);
    let l = Ledger::new(&pairs);
    let midnight = DAY0 + 86_400;
    for t in [midnight - 10, midnight - 9] {
        let seq = l.admitted(t, "a", 1_000);
        l.finish(t, seq, "answered", Some(10));
    }
    assert_eq!(
        reason(&l.admit(midnight - 1, "b", 1_000)),
        ("spending_limit".into(), "daily_call_cap".into())
    );
    l.admitted(midnight, "b", 1_000);
    assert_eq!(l.stats(midnight).calls_today, 1);
}

#[test]
fn distinct_ip_cap_with_its_prune_throttle() {
    let l = Ledger::generous();
    let seed = l.admitted(1_000, "seed", 1_000);
    l.finish(1_000, seed, "answered", Some(1));
    let mut sql_text = String::from("BEGIN;");
    for i in 0..4_095 {
        sql_text.push_str(&format!(
            "INSERT INTO ip_windows VALUES ('k{i}', '1000', 1000);"
        ));
    }
    sql_text.push_str("UPDATE meta SET ip_count = 4096, ips_pruned_at = 0; COMMIT;");
    l.exec(&sql_text);
    l.host.evict();
    l.executed();

    // At the cap a new key prunes (nothing stale yet) and is refused.
    assert_eq!(
        reason(&l.admit(1_010, "new1", 1_000)),
        ("request_limit".into(), "traffic_limit".into())
    );
    assert!(l.executed().contains(&sql::IP_PRUNE));
    assert_eq!(l.meta().ips_pruned_at, 1_010);
    // Within 60 s no prune runs again.
    assert_eq!(reason(&l.admit(1_069, "new2", 1_000)).1, "traffic_limit");
    assert!(!l.executed().contains(&sql::IP_PRUNE));
    // A known key is not affected by the cap.
    l.admitted(1_069, "k7", 1_000);
    // A day later every old key is stale: the throttled prune frees room.
    l.admitted(1_000 + 86_401, "new3", 1_000);
    assert_eq!(l.count("SELECT COUNT(*) FROM ip_windows"), 2);
    assert_eq!(l.meta().ip_count, 2);
}

#[test]
fn denied_admits_write_nothing() {
    let l = Ledger::new(&[]);
    let seq = l.admitted(DAY0, "a", 400_000_000);
    l.admitted(DAY0, "b", 1_000);
    let before = l.snapshot();
    l.executed();
    // busy, spending, invalid reservation, per-IP window: no write at all.
    assert_eq!(reason(&l.admit(DAY0 + 1, "c", 1_000)).0, "busy");
    l.finish(DAY0 + 1, seq, "timeout", None);
    let before_finish = l.snapshot();
    l.executed();
    assert_eq!(reason(&l.admit(DAY0 + 2, "c", 200_000_000)).1, "minute");
    assert_eq!(reason(&l.admit(DAY0 + 2, "c", 0)).1, "invalid_reservation");
    assert_eq!(
        reason(&l.admit(DAY0 + 2, "c", 1_000_000_000_000_001)).1,
        "invalid_reservation"
    );
    for statement in l.executed() {
        let upper = statement.trim_start().to_ascii_uppercase();
        assert!(upper.starts_with("SELECT"), "{statement}");
    }
    assert_eq!(l.snapshot(), before_finish);
    assert_ne!(before, before_finish);
}

#[test]
fn budget_config_problems_deny_with_budget_unavailable() {
    let l = Ledger::new(&[("AI_DAY_BUDGET_USD", "-1")]);
    assert_eq!(
        reason(&l.admit(DAY0, "a", 1_000)),
        ("budget_unavailable".into(), "invalid_budget".into())
    );
    match l.op(DAY0, LedgerOp::Ping) {
        LedgerReply::Pong { ledger_ok, .. } => assert!(!ledger_ok),
        other => panic!("{other:?}"),
    }
    let l = Ledger::new(&[("AI_MINUTE_BUDGET_USD", "0")]);
    assert_eq!(reason(&l.admit(DAY0, "a", 1)).1, "minute");
}

// ---------------------------------------------------------------- commands

#[test]
fn command_window_and_daily_cap_with_rollover() {
    // The daily cap (120, the minimum), spread out so that the window never fills.
    let l = Ledger::new(&[("COMMAND_LOG_DAILY_CAP", "120")]);
    for i in 0..120 {
        assert_eq!(l.command(DAY0 + i, "ls"), LedgerReply::Done);
    }
    assert_eq!(
        l.command(DAY0 + 120, "ls"),
        LedgerReply::Refused {
            reason: "daily_cap".into()
        }
    );
    // A refusal writes nothing.
    assert_eq!(l.count("SELECT COUNT(*) FROM commands"), 120);
    // Next UTC day.
    assert_eq!(l.command(DAY0 + 86_400, "ls"), LedgerReply::Done);
    assert_eq!(l.meta().cmd_today, 1);

    // The window alone, spread out: 2 per second for 60 s, then one second later 2 places free.
    let l = Ledger::new(&[]);
    for i in 0..120 {
        l.command(DAY0 + i / 2, "ls");
    }
    assert_eq!(
        l.command(DAY0 + 59, "x"),
        LedgerReply::Refused {
            reason: "window_full".into()
        }
    );
    assert_eq!(l.command(DAY0 + 60, "x"), LedgerReply::Done);
    assert_eq!(l.command(DAY0 + 60, "x"), LedgerReply::Done);
    assert_eq!(
        l.command(DAY0 + 60, "x"),
        LedgerReply::Refused {
            reason: "window_full".into()
        }
    );
    assert_eq!(
        l.op(
            DAY0,
            LedgerOp::Command(CommandOp {
                command: "x".into(),
                command_len: 1,
                mode: "AI".into(),
                ip: "x".into()
            })
        ),
        LedgerReply::error("invalid_op")
    );
}

#[test]
fn command_window_reloads_after_a_wake() {
    let l = Ledger::new(&[]);
    for i in 0..120 {
        l.command(DAY0 + i / 4, "ls");
    }
    l.host.evict();
    assert_eq!(
        l.command(DAY0 + 40, "ls"),
        LedgerReply::Refused {
            reason: "window_full".into()
        }
    );
    // Seqs keep growing across the wake.
    assert_eq!(l.command(DAY0 + 60, "ls"), LedgerReply::Done);
    assert_eq!(l.count("SELECT MAX(seq) FROM commands"), 121);
    // An old window is not reloaded at all.
    l.host.evict();
    l.executed();
    assert_eq!(l.command(DAY0 + 1_000, "ls"), LedgerReply::Done);
    assert!(!l.executed().contains(&sql::COMMAND_RECENT_AT));
}

#[test]
fn commands_get_seqs_even_without_meta() {
    let l = Ledger::new(&[]);
    l.command(DAY0, "ls");
    l.command(DAY0, "ls");
    l.exec("DELETE FROM meta");
    l.host.evict();
    // Meta missing, no question row: a fresh book that continues the command seqs.
    assert_eq!(l.command(DAY0 + 1, "ls"), LedgerReply::Done);
    assert_eq!(l.count("SELECT MAX(seq) FROM commands"), 3);
    l.admitted(DAY0 + 2, "a", 1_000);
}

// ---------------------------------------------------------------- import

#[test]
fn id_keyed_import() {
    let l = Ledger::generous();
    let now = DAY0;
    let live = l.admitted(now - 10, "a", 1_000_000);
    l.finish(now - 10, live, "answered", Some(500_000));
    let chunk = json!([
        {"id":"v1","at":now - 100,"usd":0.5},
        {"id":"v2","at":now - 7_200,"usd":0.000873},
        {"id":"old","at":now - MONTH,"usd":1.0},
        {"id":"future","at":now + 500,"usd":0.25},
    ]);
    assert_eq!(
        l.import(now, chunk.clone()),
        LedgerReply::Imported {
            inserted: 3,
            updated: 0,
            skipped: 1
        }
    );
    assert_eq!(
        l.sums(now),
        [250_500_000, 750_500_000, 751_373_000, 751_373_000]
    );
    // The retried chunk only updates.
    assert_eq!(
        l.import(now, chunk),
        LedgerReply::Imported {
            inserted: 0,
            updated: 3,
            skipped: 1
        }
    );
    // A later copy of the VPS file: a settled amount, a new id, in any order.
    assert_eq!(
        l.import(
            now + 5,
            json!([{"id":"v3","at":now - 3_000,"usd":0.1},{"id":"v1","at":now - 100,"usd":0.2}])
        ),
        LedgerReply::Imported {
            inserted: 1,
            updated: 1,
            skipped: 0
        }
    );
    assert_eq!(l.sums(now + 5)[1], 550_500_000);
    assert_eq!(l.stats(now + 5).imported_rows, 4);
    let future_at = l.count("SELECT at FROM questions WHERE qid = 'future'");
    assert_eq!(future_at, now);
    assert_eq!(l.count("SELECT state FROM questions WHERE qid = 'v1'"), 9);
    // Imported charges count against the windows like live ones.
    let sums = l.meta().sums;
    l.op(now + 5, LedgerOp::Rebuild);
    assert_eq!(l.meta().sums, sums);
    // Closed import refused.
    assert_eq!(l.op(now + 6, LedgerOp::ImportClose), LedgerReply::Done);
    assert_eq!(
        l.import(now + 6, json!([{"id":"v9","at":now,"usd":0.1}])),
        LedgerReply::error("import_closed")
    );
    let l = Ledger::generous();
    let chunk = |n: usize| -> Value {
        (0..n)
            .map(|i| json!({"id":format!("x{i}"),"at":now,"usd":0.0}))
            .collect()
    };
    assert_eq!(
        l.import(now, chunk(super::IMPORT_MAX + 1)),
        LedgerReply::error("too_many_charges")
    );
    assert_eq!(l.count("SELECT COUNT(*) FROM imported"), 0);
    assert_eq!(
        l.import(now, chunk(super::IMPORT_MAX)),
        LedgerReply::Imported {
            inserted: 25,
            updated: 0,
            skipped: 0
        }
    );
}

// ---------------------------------------------------------------- storage guard, purge, maintenance

#[test]
fn text_budget_drops_text_and_keeps_metadata() {
    let l = Ledger::new(&[("LOG_TEXT_BUDGET_MB", "1")]);
    let budget = 1_048_576;
    l.command(DAY0, "ls");
    l.exec(&format!("UPDATE meta SET text_bytes = {}", budget - 15));
    l.host.evict();
    match l.op(DAY0, LedgerOp::Ping) {
        LedgerReply::Pong {
            text_full,
            ledger_ok,
            db_bytes,
        } => {
            assert!(text_full && ledger_ok && db_bytes > 0);
        }
        other => panic!("{other:?}"),
    }
    // 12 bytes fit, the next row does not.
    let seq = l.admitted(DAY0, "a", 1_000);
    assert_eq!(l.meta().text_bytes, budget - 3);
    assert_eq!(l.finish(DAY0, seq, "answered", Some(10)), LedgerReply::Done);
    assert_eq!(l.command(DAY0, "projects"), LedgerReply::Done);
    let rows = l
        .host
        .store
        .borrow_mut()
        .rows("SELECT question, question_len, answer, answer_len FROM questions");
    assert_eq!(
        rows[0],
        vec![
            SqlValue::Text("Micro Mages?".into()),
            SqlValue::Int(12),
            SqlValue::Null,
            SqlValue::Int(7)
        ]
    );
    let command = l
        .host
        .store
        .borrow_mut()
        .rows("SELECT command, command_len FROM commands ORDER BY seq DESC LIMIT 1");
    assert_eq!(command[0], vec![SqlValue::Null, SqlValue::Int(8)]);
    let meta = l.meta();
    assert_eq!(meta.text_dropped, 2);
    assert_eq!(meta.text_bytes, budget - 3);
    assert_eq!(l.logged("💾 Ledger text budget reached"), 1);
}

#[test]
fn purge_never_deletes_questions_inside_the_month_window() {
    let l = Ledger::generous();
    let now = DAY0 + 40 * 86_400;
    l.command(now, "ls");
    l.admitted(now, "a", 1_000);
    l.exec(&format!(
        "INSERT INTO questions (seq, qid, at, nusd, state, lease_until, question, answer) VALUES
           (100, 'old', {old}, 5, 1, 0, 'abcd', 'ef'),
           (101, 'recent', {recent}, 5, 1, 0, 'x', NULL);
         INSERT INTO commands (seq, at, command, command_len, mode, ip) VALUES
           (100, {old}, 'old', 3, 'classic', 'x');
         UPDATE meta SET text_bytes = text_bytes + 10",
        old = now - 31 * 86_400,
        recent = now - 29 * 86_400
    ));
    l.host.evict();
    let text_before = l.meta().text_bytes;
    let purged = l.op(
        now,
        LedgerOp::Purge(PurgeOp {
            table: "questions".into(),
            before: now,
        }),
    );
    assert_eq!(
        purged,
        LedgerReply::Purged {
            deleted: 1,
            more: false,
            budget_exhausted: false
        }
    );
    assert_eq!(
        l.count("SELECT COUNT(*) FROM questions WHERE qid = 'recent'"),
        1
    );
    assert_eq!(l.count("SELECT COUNT(*) FROM questions"), 2);
    assert_eq!(l.meta().text_bytes, text_before - 6);
    let purged = l.op(
        now,
        LedgerOp::Purge(PurgeOp {
            table: "commands".into(),
            before: now - 86_400,
        }),
    );
    assert_eq!(
        purged,
        LedgerReply::Purged {
            deleted: 1,
            more: false,
            budget_exhausted: false
        }
    );
    assert_eq!(l.count("SELECT COUNT(*) FROM commands"), 1);
    assert_eq!(
        l.op(
            now,
            LedgerOp::Purge(PurgeOp {
                table: "meta".into(),
                before: now
            })
        ),
        LedgerReply::error("invalid_table")
    );
}

#[test]
fn maintenance_batches_and_backlog_rerun() {
    let l = Ledger::new(&[("LOG_RETENTION_DAYS", "31")]);
    let now = DAY0 + 60 * 86_400;
    l.admitted(now - 7_200, "a", 1_000);
    let old = now - 40 * 86_400;
    let mut sql_text = String::from("BEGIN;");
    for i in 0..300 {
        sql_text.push_str(&format!(
            "INSERT INTO commands (seq, at, command, command_len, mode, ip) VALUES ({}, {old}, 'abc', 3, 'classic', 'x');",
            1_000 + i
        ));
    }
    for i in 0..260 {
        sql_text.push_str(&format!(
            "INSERT INTO questions (seq, qid, at, nusd, state, lease_until, question) VALUES ({}, 'q{i}', {old}, 5, 1, 0, 'ab');",
            1_000 + i
        ));
    }
    sql_text.push_str(&format!(
        "UPDATE meta SET cmd_next_seq = 2000, next_seq = 2000, text_bytes = text_bytes + 1420, maintained_at = {}; COMMIT;",
        now - 3_600
    ));
    l.exec(&sql_text);
    l.host.evict();

    // First pass: full batches of 250, then a re-run 60 s later.
    assert_eq!(l.command(now, "ls"), LedgerReply::Done);
    assert_eq!(
        l.count(&format!("SELECT COUNT(*) FROM commands WHERE at = {old}")),
        50
    );
    assert_eq!(
        l.count(&format!("SELECT COUNT(*) FROM questions WHERE at = {old}")),
        10
    );
    assert_eq!(l.meta().maintained_at, now - 3_540);
    assert_eq!(l.command(now + 59, "ls"), LedgerReply::Done);
    assert_eq!(
        l.count(&format!("SELECT COUNT(*) FROM commands WHERE at = {old}")),
        50
    );
    assert_eq!(l.command(now + 60, "ls"), LedgerReply::Done);
    assert_eq!(
        l.count(&format!("SELECT COUNT(*) FROM commands WHERE at = {old}")),
        0
    );
    assert_eq!(
        l.count(&format!("SELECT COUNT(*) FROM questions WHERE at = {old}")),
        0
    );
    assert_eq!(l.meta().maintained_at, now + 60);
    // 300 × 3 + 260 × 2 bytes freed: the live question and the three new commands remain.
    assert_eq!(l.meta().text_bytes, 12 + 2 + 2 + 2);
    assert_eq!(l.logged("🧹 Ledger maintenance"), 2);

    // Retention 0 (the default) never deletes logs.
    let keep = Ledger::new(&[]);
    keep.command(now - 7_200, "ls");
    keep.exec(&format!(
        "INSERT INTO commands (seq, at, command, command_len, mode, ip) VALUES (500, {old}, 'abc', 3, 'classic', 'x'); UPDATE meta SET cmd_next_seq = 501, maintained_at = {}",
        now - 3_600
    ));
    keep.host.evict();
    keep.command(now, "ls");
    assert_eq!(
        keep.count(&format!("SELECT COUNT(*) FROM commands WHERE at = {old}")),
        1
    );
    assert_eq!(keep.logged("🧹 Ledger maintenance"), 1);
}

/// Old rows for the delete tests: `commands` and `questions` rows at `old`, seqs from 1,000.
fn old_rows(l: &Ledger, old: i64, commands: usize, questions: usize, maintained_at: i64) {
    let mut sql_text = String::from("BEGIN;");
    for i in 0..commands {
        sql_text.push_str(&format!(
            "INSERT INTO commands (seq, at, command, command_len, mode, ip) VALUES ({}, {old}, 'abc', 3, 'classic', 'x');",
            1_000 + i
        ));
    }
    for i in 0..questions {
        sql_text.push_str(&format!(
            "INSERT INTO questions (seq, qid, at, nusd, state, lease_until, question) VALUES ({}, 'q{i}', {old}, 5, 1, 0, 'ab');",
            1_000 + i
        ));
    }
    sql_text.push_str(&format!(
        "UPDATE meta SET cmd_next_seq = 5000, next_seq = 5000, maintained_at = {maintained_at}; COMMIT;"
    ));
    l.exec(&sql_text);
    l.host.evict();
}

#[test]
fn purges_stop_at_the_daily_delete_budget() {
    // 1,000 rows written per UTC day: 500 questions (row + index entry) or 1,000 commands.
    let l = Ledger::new(&[("LOG_DELETE_DAILY_ROWS", "1000")]);
    let now = DAY0 + 60 * 86_400;
    l.admitted(now - 7_200, "a", 1_000);
    let old = now - 40 * 86_400;
    old_rows(&l, old, 1_500, 600, now);
    let purge = |table: &str, at: i64, before: i64| {
        l.op(
            at,
            LedgerOp::Purge(PurgeOp {
                table: table.into(),
                before,
            }),
        )
    };
    let old_count =
        |table: &str| l.count(&format!("SELECT COUNT(*) FROM {table} WHERE at = {old}"));

    assert_eq!(
        purge("questions", now, now),
        LedgerReply::Purged {
            deleted: 500,
            more: true,
            budget_exhausted: true
        }
    );
    assert_eq!(old_count("questions"), 100);
    let s = l.stats(now + 1);
    assert_eq!((s.delete_rows_today, s.delete_daily_rows), (1_000, 1_000));
    // Used up for today, whatever the table: nothing is deleted or written.
    let before = l.snapshot();
    assert_eq!(
        purge("commands", now + 10, old),
        LedgerReply::Purged {
            deleted: 0,
            more: true,
            budget_exhausted: true
        }
    );
    assert_eq!(l.snapshot(), before);
    assert_eq!(l.logged("🧹 Daily delete budget used"), 1);
    // The next UTC day has a fresh budget.
    let tomorrow = now + 86_400;
    assert_eq!(l.stats(tomorrow).delete_rows_today, 0);
    assert_eq!(
        purge("commands", tomorrow, old),
        LedgerReply::Purged {
            deleted: 1_000,
            more: true,
            budget_exhausted: true
        }
    );
    let after = tomorrow + 86_400;
    assert_eq!(
        purge("commands", after, old),
        LedgerReply::Purged {
            deleted: 500,
            more: false,
            budget_exhausted: false
        }
    );
    assert_eq!(old_count("commands"), 0);
    assert_eq!(l.stats(after).delete_rows_today, 500);
    // With room left in today's budget, a purge that runs out of rows is simply done.
    assert_eq!(
        purge("questions", after, after),
        LedgerReply::Purged {
            deleted: 100,
            more: false,
            budget_exhausted: false
        }
    );
    assert_eq!(l.stats(after).delete_rows_today, 700);
}

#[test]
fn retention_shares_the_daily_delete_budget() {
    let l = Ledger::new(&[
        ("LOG_RETENTION_DAYS", "31"),
        ("LOG_DELETE_DAILY_ROWS", "1000"),
    ]);
    let now = DAY0 + 60 * 86_400;
    l.admitted(now - 7_200, "a", 1_000);
    let old = now - 40 * 86_400;
    old_rows(&l, old, 1_000, 400, now - 3_600);
    let old_count =
        |table: &str| l.count(&format!("SELECT COUNT(*) FROM {table} WHERE at = {old}"));

    // 250 questions (500 rows) and 250 commands: 750 of 1,000 used, back in 60 s.
    assert_eq!(l.command(now, "ls"), LedgerReply::Done);
    assert_eq!((old_count("questions"), old_count("commands")), (150, 750));
    assert_eq!(l.meta().maintained_at, now - 3_540);
    // 125 questions use the rest: no 60 s re-run while the budget is used up.
    assert_eq!(l.command(now + 60, "ls"), LedgerReply::Done);
    assert_eq!((old_count("questions"), old_count("commands")), (25, 750));
    assert_eq!(l.meta().maintained_at, now + 60);
    assert_eq!(l.logged("🧹 Daily delete budget used"), 1);
    // An hour later, the same UTC day: nothing deleted.
    assert_eq!(l.command(now + 3_660, "ls"), LedgerReply::Done);
    assert_eq!((old_count("questions"), old_count("commands")), (25, 750));
    assert_eq!(l.meta().maintained_at, now + 3_660);
    assert_eq!(l.stats(now + 3_661).delete_rows_today, 1_000);
    let lines: Vec<Value> = l
        .logs
        .borrow()
        .iter()
        .filter(|(_, msg, _)| *msg == "🧹 Ledger maintenance")
        .map(|(_, _, fields)| fields.clone())
        .collect();
    assert_eq!(lines.len(), 3);
    assert_eq!(lines[0]["delete_budget_left"], 250);
    assert_eq!(lines[1]["delete_budget_left"], 0);
    assert_eq!(lines[2]["deleted_commands"], 0);
    // The next UTC day: the rest of the questions and a new backlog run of commands.
    let tomorrow = now + 86_400;
    assert_eq!(l.command(tomorrow, "ls"), LedgerReply::Done);
    assert_eq!((old_count("questions"), old_count("commands")), (0, 500));
    assert_eq!(l.meta().maintained_at, tomorrow - 3_540);
    assert_eq!(l.stats(tomorrow).delete_rows_today, 25 * 2 + 250);
}

// ---------------------------------------------------------------- atomicity

#[test]
fn commit_failure_leaves_storage_unchanged_and_drops_the_book() {
    let l = Ledger::generous();
    l.admitted(DAY0, "a", 1_000);
    let before = l.snapshot();
    l.host.store.borrow_mut().fail_next_commit = true;
    let denied = l.admit(DAY0 + 1, "b", 1_000);
    assert_eq!(
        reason(&denied),
        ("spending_limit".into(), "ledger_write_failed".into())
    );
    assert_eq!(l.snapshot(), before);
    assert!(!l.host.cached());
    assert_eq!(l.logged("🧾 Ledger transaction failed"), 1);
    // A failure after the first statement rolls the earlier ones back too.
    l.host.store.borrow_mut().fail_statement = Some("INSERT INTO ip_windows");
    assert_eq!(
        reason(&l.admit(DAY0 + 2, "c", 1_000)).1,
        "ledger_write_failed"
    );
    assert_eq!(l.snapshot(), before);
    // The reloaded book carries on from storage.
    assert_eq!(l.admitted(DAY0 + 3, "d", 1_000), 2);
    // Other ops report the failure as an error.
    l.host.store.borrow_mut().fail_next_commit = true;
    assert_eq!(l.command(DAY0 + 4, "ls"), LedgerReply::error("storage"));
    assert_eq!(l.count("SELECT COUNT(*) FROM commands"), 0);
}

#[test]
fn rebuild_reproduces_the_incremental_sums() {
    let l = Ledger::generous();
    let mut t = DAY0;
    let mut open = Vec::new();
    for i in 0..40i64 {
        t += 37 * (i % 7) + 11;
        let seq = l.admitted(t, &format!("ip{}", i % 5), 1_000 + i * 997);
        match i % 4 {
            0 => {
                l.finish(t, seq, "answered", Some(i * 13));
            }
            1 => {
                l.finish(t, seq, "provider_unavailable", None);
            }
            2 => {
                l.finish(t + 1, seq, "timeout", Some(5));
            }
            _ => open.push(seq),
        }
        if open.len() == 2 {
            for seq in open.drain(..) {
                l.finish(t, seq, "answered", Some(1));
            }
        }
    }
    l.import(
        t,
        json!([{"id":"x","at":t - 4_000,"usd":0.003},{"id":"y","at":t - 30,"usd":0.0001}]),
    );
    let check_at = t + 1_800;
    l.stats(check_at);
    let incremental = l.host.book.borrow().as_ref().unwrap().meta().clone();
    assert_eq!(l.op(check_at, LedgerOp::Rebuild), LedgerReply::Done);
    let rebuilt = l.meta();
    assert_eq!(rebuilt.sums, incremental.sums);
    assert_eq!(rebuilt.edges, incremental.edges);
    assert_eq!(rebuilt.next_seq, incremental.next_seq);
    assert_eq!(rebuilt.calls_today, incremental.calls_today);
    assert_eq!(rebuilt.ip_count, incremental.ip_count);
}

// ---------------------------------------------------------------- export

#[test]
fn export_pages_lines_and_cursors() {
    let l = Ledger::generous();
    let a = l.admitted(DAY0, "198.51.100.1", 2_532_000);
    l.finish(DAY0 + 1, a, "answered", Some(20_000));
    let b = l.admitted(DAY0 + 2, "198.51.100.2", 2_532_000);
    l.finish(DAY0 + 3, b, "provider_unavailable", None);
    let c = l.admitted(DAY0 + 4, "198.51.100.3", 2_532_000);
    l.import(
        DAY0 + 4,
        json!([{"id":"vps-1","at":DAY0 - 10,"usd":0.000873}]),
    );
    let d = l.admitted(DAY0 + 5, "198.51.100.4", 1_000);

    // c is in flight: the page stops before it and hands out the cursor after b.
    let page = l.export(DAY0 + 6, "questions", "0", None);
    assert_eq!(page.content_type, "application/x-ndjson");
    let text = String::from_utf8(page.body.clone()).unwrap();
    let types: Vec<String> = text
        .lines()
        .map(|line| {
            serde_json::from_str::<Value>(line).unwrap()["entry_type"]
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect();
    assert_eq!(
        types,
        [
            "ai_question",
            "ai_answer",
            "ai_usage",
            "ai_question",
            "ai_failure"
        ]
    );
    assert_eq!(page.next_after.as_deref(), Some(b.to_string().as_str()));
    let blocked = l.export(DAY0 + 6, "questions", &b.to_string(), None);
    assert!(blocked.body.is_empty());
    assert_eq!(blocked.next_after, None);
    // Once the lease has expired the row is exported as abandoned; imported rows give no line.
    let later = l.export(DAY0 + 4 + 20, "questions", &b.to_string(), Some(2));
    let text = String::from_utf8(later.body).unwrap();
    assert!(text.contains(r#""reason":"abandoned""#));
    assert_eq!(
        later.next_after.as_deref(),
        Some((c + 1).to_string().as_str())
    );
    let last = l.export(DAY0 + 30, "questions", &(c + 1).to_string(), Some(5));
    assert_eq!(String::from_utf8(last.body).unwrap().lines().count(), 2);
    assert_eq!(last.next_after, None);
    let _ = d;

    // Ledger rows of the last 30 days in (at, seq) order, VPS shape.
    let ledger = l.export(DAY0 + 30, "ledger", "0", Some(2));
    let text = String::from_utf8(ledger.body).unwrap();
    assert_eq!(
        text.lines().next().unwrap(),
        format!(r#"{{"id":"vps-1","at":{},"usd":0.000873}}"#, DAY0 - 10)
    );
    let cursor = ledger.next_after.unwrap();
    assert_eq!(cursor, format!("{}:{a}", DAY0));
    let rest = l.export(DAY0 + 30, "ledger", &cursor, Some(10));
    let ids: Vec<String> = String::from_utf8(rest.body)
        .unwrap()
        .lines()
        .map(|line| {
            serde_json::from_str::<Value>(line).unwrap()["id"]
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect();
    assert_eq!(ids.len(), 3);
    assert_eq!(rest.next_after, None);

    // Commands, and invalid input.
    l.command(DAY0, "projects");
    let commands = l.export(DAY0, "commands", "0", None);
    assert!(String::from_utf8(commands.body)
        .unwrap()
        .contains(r#""entry_type":"command","command":"projects""#));
    let bad: LedgerReply = serde_json::from_slice(&l.export(DAY0, "meta", "0", None).body).unwrap();
    assert_eq!(bad, LedgerReply::error("invalid_table"));
    let bad: LedgerReply =
        serde_json::from_slice(&l.export(DAY0, "ledger", "12", None).body).unwrap();
    assert_eq!(bad, LedgerReply::error("invalid_cursor"));
}

#[test]
fn export_pages_stop_at_256_kib() {
    let l = Ledger::new(&[]);
    let long = "é".repeat(200);
    for i in 0..1_000 {
        l.op(
            DAY0 + i,
            LedgerOp::Command(CommandOp {
                command: long.clone(),
                command_len: 200,
                mode: "ai".into(),
                ip: "203.0.113.7".into(),
            }),
        );
    }
    let page = l.export(DAY0 + 1_100, "commands", "0", None);
    assert!(page.body.len() <= super::export::PAGE_BYTES);
    assert!(page.body.len() > super::export::PAGE_BYTES - 1_000);
    let rows = String::from_utf8(page.body).unwrap().lines().count();
    assert!(rows < 1_000);
    assert_eq!(page.next_after, Some(rows.to_string()));
}

// ---------------------------------------------------------------- stats, protocol errors

#[test]
fn stats_are_bounded_and_complete() {
    let l = Ledger::generous();
    l.admitted(DAY0, "unknown", 1_000);
    l.op(
        DAY0,
        LedgerOp::Command(CommandOp {
            command: "ls".into(),
            command_len: 2,
            mode: "classic".into(),
            ip: "unknown".into(),
        }),
    );
    l.command(DAY0, "ls");
    let s = l.stats(DAY0 + 1);
    assert!(s.valid && s.budget_valid);
    assert_eq!(s.questions_rows, 1);
    assert_eq!(s.commands_rows, 2);
    assert_eq!(s.unknown_ip_questions_24h, 1);
    assert_eq!(s.unknown_ip_commands_last_10k, 1);
    assert_eq!(s.calls_today, 1);
    assert_eq!(s.cmd_today, 2);
    assert_eq!(s.inflight.len(), 1);
    assert_eq!(s.limits_usd.minute, Some(1000.0));
    assert_eq!(s.reader_version, 1);
    assert!(s.db_bytes > 0);
    assert_eq!(s.text_budget_bytes, 256 * 1_048_576);
}

#[test]
fn malformed_and_unknown_ops() {
    let l = Ledger::generous();
    let reply =
        |body: &[u8]| -> LedgerReply { serde_json::from_slice(&l.raw(DAY0, body).body).unwrap() };
    assert_eq!(reply(b"not json"), LedgerReply::error("invalid_op"));
    assert_eq!(
        reply(br#"{"op":"admit"}"#),
        LedgerReply::error("invalid_op")
    );
    assert_eq!(
        reply(br#"{"op":"compact"}"#),
        LedgerReply::error("unsupported_op")
    );
    assert_eq!(reply(br#"{"op":"ping","extra":1}"#).status(), "pong");
}

// ---------------------------------------------------------------- query plans

#[test]
fn sql_plans_never_scan_questions_or_commands() {
    let store = super::sqlite_store::SqliteStore::open();
    let statements: [(&str, &str, usize); 22] = [
        ("ip prune", sql::IP_PRUNE, 1),
        ("ip count", sql::IP_COUNT, 0),
        ("questions retention", sql::QUESTIONS_DELETE_UPTO, 2),
        ("commands retention", sql::COMMANDS_DELETE_UPTO, 2),
        ("export questions", sql::EXPORT_QUESTIONS, 2),
        ("export commands", sql::EXPORT_COMMANDS, 2),
        ("export ledger", sql::EXPORT_LEDGER, 5),
        ("questions max", sql::QUESTIONS_MAX_SEQ, 0),
        ("questions min", sql::QUESTIONS_MIN_SEQ, 0),
        ("commands max", sql::COMMANDS_MAX_SEQ, 0),
        ("commands min", sql::COMMANDS_MIN_SEQ, 0),
        ("imported count", sql::IMPORTED_COUNT, 0),
        ("unknown ip questions", sql::UNKNOWN_IP_QUESTIONS, 1),
        ("unknown ip commands", sql::UNKNOWN_IP_COMMANDS, 1),
        ("window expiry", sql::SUM_RANGE, 2),
        ("rebuild sums", sql::SUM_AFTER, 1),
        ("rebuild inflight", sql::INFLIGHT_ROWS, 2),
        ("rebuild calls", sql::CALLS_SINCE, 1),
        ("rebuild commands", sql::COMMANDS_SINCE, 2),
        ("recent commands", sql::COMMAND_RECENT_AT, 1),
        ("question head", sql::QUESTION_HEAD, 1),
        ("ip hits", sql::IP_HITS, 1),
    ];
    // LIMIT-bounded reads in seq order are the only scans allowed.
    let bounded_scans = ["commands retention", "recent commands"];
    let mut report = Vec::new();
    for (name, statement, params) in statements {
        let plan = store.plan(statement, params);
        report.push(format!("{name}: {plan:?}"));
        for detail in &plan {
            let scans = detail.starts_with("SCAN questions") || detail.starts_with("SCAN commands");
            if scans {
                assert!(
                    bounded_scans.contains(&name) && detail == "SCAN commands",
                    "{name} scans: {plan:?}"
                );
            }
        }
    }
    // The retention delete of questions goes through the covering index.
    let plan = store.plan(sql::QUESTIONS_DELETE_UPTO, 2);
    assert!(
        plan.iter()
            .any(|d| d.contains("USING COVERING INDEX questions_at_nusd (at<?)")),
        "{plan:?}"
    );
    let plan = store.plan(sql::SUM_RANGE, 2);
    assert!(
        plan.iter()
            .any(|d| d.contains("USING COVERING INDEX questions_at_nusd (at>? AND at<?)")),
        "{plan:?}"
    );
    println!("{}", report.join("\n"));
}

#[test]
fn extreme_stored_values_never_panic() {
    let l = Ledger::generous();
    l.admitted(DAY0, "a", 1_000);
    l.command(DAY0, "ls");
    l.exec(
        "UPDATE meta SET next_seq = 9223372036854775807, cmd_next_seq = 9223372036854775807, \
         ip_count = 9223372036854775807, ips_pruned_at = -9223372036854775808, \
         maintained_at = -9223372036854775808, calls_today = 9223372036854775807, \
         cmd_last_at = 9223372036854775807; \
         UPDATE ip_windows SET hits = '-9223372036854775808 9223372036854775807';",
    );
    l.host.evict();
    // Whatever the replies, nothing traps (debug builds check overflow).
    let _ = l.admit(DAY0 + 1, "a", 1_000);
    let _ = l.admit(DAY0 + 1, "b", 1_000);
    let _ = l.command(DAY0 + 1, "ls");
    let _ = l.command(DAY0 + 2, "ls");
    let _ = l.stats(DAY0 + 3);
    let _ = l.export(DAY0 + 3, "commands", "0", None);
    let _ = l.op(DAY0 + 3, LedgerOp::Rebuild);
    let _ = l.admit(DAY0 + 4, "c", 1_000);
}
