//! Admin export: NDJSON lines in the legacy JSON-lines shapes (`command`, `ai_question`,
//! `ai_answer`, field order of the original structs) plus `ai_usage` and `ai_failure`, and the
//! VPS ledger shape `{"id","at","usd"}`.

use serde::Serialize;
use serde_json::Value;

use crate::core::money::usd;
use crate::core::time::rfc3339;

/// Every page stops at this many bytes (a row's lines are never split).
pub const PAGE_BYTES: usize = 262_144;
/// Rows per page: questions (up to 3 lines each) and the other tables.
pub const MAX_QUESTION_ROWS: i64 = 200;
pub const MAX_ROWS: i64 = 1_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Table {
    Questions,
    Commands,
    Ledger,
}

impl Table {
    pub fn parse(name: &str) -> Option<Table> {
        match name {
            "questions" => Some(Table::Questions),
            "commands" => Some(Table::Commands),
            "ledger" => Some(Table::Ledger),
            _ => None,
        }
    }

    pub fn max_rows(self) -> i64 {
        match self {
            Table::Questions => MAX_QUESTION_ROWS,
            Table::Commands | Table::Ledger => MAX_ROWS,
        }
    }

    /// `limit` clamped to `1..=max_rows` (missing: the maximum).
    pub fn page_rows(self, limit: Option<i64>) -> i64 {
        limit.unwrap_or(self.max_rows()).clamp(1, self.max_rows())
    }
}

/// Export cursor (`after`): a seq for `questions` and `commands`, `"<at>:<seq>"` for `ledger`;
/// `"0"` starts every table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cursor {
    Seq(i64),
    AtSeq(i64, i64),
    Start,
}

pub fn parse_cursor(table: Table, after: &str) -> Option<Cursor> {
    if after == "0" {
        return Some(Cursor::Start);
    }
    match table {
        Table::Questions | Table::Commands => after
            .parse::<i64>()
            .ok()
            .filter(|seq| *seq >= 0)
            .map(Cursor::Seq),
        Table::Ledger => {
            let (at, seq) = after.split_once(':')?;
            Some(Cursor::AtSeq(at.parse().ok()?, seq.parse().ok()?))
        }
    }
}

pub fn format_cursor(cursor: Cursor) -> String {
    match cursor {
        Cursor::Start => "0".to_string(),
        Cursor::Seq(seq) => seq.to_string(),
        Cursor::AtSeq(at, seq) => format!("{at}:{seq}"),
    }
}

/// One `questions` row as exported.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct QuestionRow {
    pub seq: i64,
    pub qid: String,
    pub at: i64,
    pub nusd: i64,
    pub state: i64,
    pub lease_until: i64,
    pub ip: Option<String>,
    pub question: Option<String>,
    pub question_len: Option<i64>,
    pub answer_id: Option<String>,
    pub answer: Option<String>,
    pub answer_len: Option<i64>,
    pub model: Option<String>,
    pub finished_at: Option<i64>,
    pub usage: Option<String>,
    pub latency_ms: Option<i64>,
    pub error: Option<String>,
}

pub const STATE_INFLIGHT: i64 = 0;
pub const STATE_ANSWERED: i64 = 1;
pub const STATE_PROVIDER_UNAVAILABLE: i64 = 2;
pub const STATE_TIMEOUT: i64 = 3;
pub const STATE_IMPORTED: i64 = 9;

#[derive(Serialize)]
struct QuestionLine<'a> {
    timestamp: String,
    entry_type: &'static str,
    question_id: &'a str,
    question: Option<&'a str>,
    question_len: i64,
    ip: Option<&'a str>,
}

#[derive(Serialize)]
struct AnswerLine<'a> {
    timestamp: String,
    entry_type: &'static str,
    question_id: &'a str,
    answer_id: Option<&'a str>,
    answer: Option<&'a str>,
    answer_len: i64,
    model: Option<&'a str>,
    ai_enabled: bool,
    reason: Option<&'a str>,
    ip: Option<&'a str>,
}

#[derive(Serialize)]
struct UsageLine<'a> {
    timestamp: String,
    entry_type: &'static str,
    question_id: &'a str,
    model: Option<&'a str>,
    cost_usd: f64,
    usage: Value,
    latency_ms: Option<i64>,
}

#[derive(Serialize)]
struct FailureLine<'a> {
    timestamp: String,
    entry_type: &'static str,
    question_id: &'a str,
    reason: &'a str,
    error: Option<&'a str>,
    cost_usd: f64,
    usage: Value,
    latency_ms: Option<i64>,
    ip: Option<&'a str>,
}

#[derive(Serialize)]
struct CommandLine<'a> {
    timestamp: String,
    entry_type: &'static str,
    command: Option<&'a str>,
    command_len: i64,
    mode: &'a str,
    ip: &'a str,
}

#[derive(Serialize)]
struct LedgerLine<'a> {
    id: &'a str,
    at: i64,
    usd: f64,
}

fn push_line<T: Serialize>(out: &mut Vec<u8>, line: &T) {
    if serde_json::to_writer(&mut *out, line).is_ok() {
        out.push(b'\n');
    }
}

fn usage_value(raw: Option<&str>) -> Value {
    raw.and_then(|text| serde_json::from_str(text).ok())
        .unwrap_or(Value::Null)
}

/// Whether a question row can be exported at `now` (a call still in flight cannot).
pub fn exportable(row: &QuestionRow, now: i64) -> bool {
    !(row.state == STATE_INFLIGHT && row.lease_until > now)
}

/// The lines of one question row: none for imported charges; `ai_question` then `ai_answer` +
/// `ai_usage` (answered) or `ai_failure` (failed or abandoned).
pub fn question_lines(row: &QuestionRow) -> Vec<u8> {
    let mut out = Vec::new();
    if row.state == STATE_IMPORTED {
        return out;
    }
    push_line(
        &mut out,
        &QuestionLine {
            timestamp: rfc3339(row.at),
            entry_type: "ai_question",
            question_id: &row.qid,
            question: row.question.as_deref(),
            question_len: row.question_len.unwrap_or(0),
            ip: row.ip.as_deref(),
        },
    );
    let finished = rfc3339(row.finished_at.unwrap_or(row.at));
    if row.state == STATE_ANSWERED {
        push_line(
            &mut out,
            &AnswerLine {
                timestamp: finished.clone(),
                entry_type: "ai_answer",
                question_id: &row.qid,
                answer_id: row.answer_id.as_deref(),
                answer: row.answer.as_deref(),
                answer_len: row.answer_len.unwrap_or(0),
                model: row.model.as_deref(),
                ai_enabled: true,
                reason: None,
                ip: row.ip.as_deref(),
            },
        );
        push_line(
            &mut out,
            &UsageLine {
                timestamp: finished,
                entry_type: "ai_usage",
                question_id: &row.qid,
                model: row.model.as_deref(),
                cost_usd: usd(row.nusd),
                usage: usage_value(row.usage.as_deref()),
                latency_ms: row.latency_ms,
            },
        );
    } else {
        let reason = match row.state {
            STATE_PROVIDER_UNAVAILABLE => "provider_unavailable",
            STATE_TIMEOUT => "timeout",
            _ => "abandoned",
        };
        push_line(
            &mut out,
            &FailureLine {
                timestamp: finished,
                entry_type: "ai_failure",
                question_id: &row.qid,
                reason,
                error: row.error.as_deref(),
                cost_usd: usd(row.nusd),
                usage: usage_value(row.usage.as_deref()),
                latency_ms: row.latency_ms,
                ip: row.ip.as_deref(),
            },
        );
    }
    out
}

/// One `command` line (legacy shape).
pub fn command_line(
    at: i64,
    command: Option<&str>,
    command_len: i64,
    mode: &str,
    ip: &str,
) -> Vec<u8> {
    let mut out = Vec::new();
    push_line(
        &mut out,
        &CommandLine {
            timestamp: rfc3339(at),
            entry_type: "command",
            command,
            command_len,
            mode,
            ip,
        },
    );
    out
}

/// One VPS ledger entry `{"id","at","usd"}`.
pub fn ledger_line(id: &str, at: i64, nusd: i64) -> Vec<u8> {
    let mut out = Vec::new();
    push_line(
        &mut out,
        &LedgerLine {
            id,
            at,
            usd: usd(nusd),
        },
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answered() -> QuestionRow {
        QuestionRow {
            seq: 1,
            qid: "q-1".into(),
            at: 1_795_478_400,
            nusd: 20_000,
            state: STATE_ANSWERED,
            lease_until: 0,
            ip: Some("203.0.113.7".into()),
            question: Some("Micro Mages?".into()),
            question_len: Some(12),
            answer_id: Some("a-1".into()),
            answer: Some("Python.".into()),
            answer_len: Some(7),
            model: Some("gpt-6-luna".into()),
            finished_at: Some(1_795_478_401),
            usage: Some(r#"{"completion_tokens":20,"prompt_tokens":100}"#.into()),
            latency_ms: Some(812),
            error: None,
        }
    }

    #[test]
    fn legacy_and_new_shapes() {
        let lines = String::from_utf8(question_lines(&answered())).unwrap();
        let expected = [
            r#"{"timestamp":"2026-11-24T00:00:00Z","entry_type":"ai_question","question_id":"q-1","question":"Micro Mages?","question_len":12,"ip":"203.0.113.7"}"#,
            r#"{"timestamp":"2026-11-24T00:00:01Z","entry_type":"ai_answer","question_id":"q-1","answer_id":"a-1","answer":"Python.","answer_len":7,"model":"gpt-6-luna","ai_enabled":true,"reason":null,"ip":"203.0.113.7"}"#,
            r#"{"timestamp":"2026-11-24T00:00:01Z","entry_type":"ai_usage","question_id":"q-1","model":"gpt-6-luna","cost_usd":0.00002,"usage":{"completion_tokens":20,"prompt_tokens":100},"latency_ms":812}"#,
        ];
        assert_eq!(lines, format!("{}\n", expected.join("\n")));

        let mut failed = answered();
        failed.state = STATE_PROVIDER_UNAVAILABLE;
        failed.nusd = 2_532_000;
        failed.error = Some("http_429".into());
        failed.usage = None;
        failed.question = None;
        let lines = String::from_utf8(question_lines(&failed)).unwrap();
        assert_eq!(
            lines.lines().nth(1).unwrap(),
            r#"{"timestamp":"2026-11-24T00:00:01Z","entry_type":"ai_failure","question_id":"q-1","reason":"provider_unavailable","error":"http_429","cost_usd":0.002532,"usage":null,"latency_ms":812,"ip":"203.0.113.7"}"#
        );
        assert!(lines.starts_with(
            r#"{"timestamp":"2026-11-24T00:00:00Z","entry_type":"ai_question","question_id":"q-1","question":null,"#
        ));

        let mut abandoned = answered();
        abandoned.state = STATE_INFLIGHT;
        abandoned.lease_until = abandoned.at + 20;
        abandoned.finished_at = None;
        assert!(!exportable(&abandoned, abandoned.at + 20 - 1));
        assert!(exportable(&abandoned, abandoned.at + 20));
        let lines = String::from_utf8(question_lines(&abandoned)).unwrap();
        assert!(lines.contains(r#""reason":"abandoned""#));
        assert!(lines.contains(r#""timestamp":"2026-11-24T00:00:00Z","entry_type":"ai_failure""#));

        let mut imported = answered();
        imported.state = STATE_IMPORTED;
        assert!(question_lines(&imported).is_empty());

        assert_eq!(
            String::from_utf8(command_line(0, Some("projects"), 8, "classic", "203.0.113.7")).unwrap(),
            "{\"timestamp\":\"1970-01-01T00:00:00Z\",\"entry_type\":\"command\",\"command\":\"projects\",\"command_len\":8,\"mode\":\"classic\",\"ip\":\"203.0.113.7\"}\n"
        );
        assert_eq!(
            String::from_utf8(ledger_line("8c0b", 1_764_000_000, 873_000)).unwrap(),
            "{\"id\":\"8c0b\",\"at\":1764000000,\"usd\":0.000873}\n"
        );
    }

    #[test]
    fn cursors_and_limits() {
        assert_eq!(parse_cursor(Table::Questions, "0"), Some(Cursor::Start));
        assert_eq!(parse_cursor(Table::Commands, "17"), Some(Cursor::Seq(17)));
        assert_eq!(parse_cursor(Table::Commands, "-1"), None);
        assert_eq!(parse_cursor(Table::Commands, "1:2"), None);
        assert_eq!(
            parse_cursor(Table::Ledger, "100:7"),
            Some(Cursor::AtSeq(100, 7))
        );
        assert_eq!(parse_cursor(Table::Ledger, "100"), None);
        assert_eq!(parse_cursor(Table::Ledger, "0"), Some(Cursor::Start));
        assert_eq!(format_cursor(Cursor::AtSeq(100, 7)), "100:7");
        assert_eq!(Table::parse("ledger"), Some(Table::Ledger));
        assert_eq!(Table::parse("meta"), None);
        assert_eq!(Table::Questions.page_rows(None), 200);
        assert_eq!(Table::Questions.page_rows(Some(5_000)), 200);
        assert_eq!(Table::Commands.page_rows(Some(0)), 1);
        assert_eq!(Table::Ledger.page_rows(Some(50)), 50);
    }
}
