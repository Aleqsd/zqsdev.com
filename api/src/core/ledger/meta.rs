//! The `meta` row: window edges and sums, counters, slot leases; its validation and codecs.

use super::store::{col_i64, SqlValue};

/// Written by this code; informational (additive schema changes raise it).
pub const SCHEMA_VERSION: i64 = 1;
/// Highest `min_reader_version` this code can read; a breaking change raises the stored value.
pub const READER_VERSION: i64 = 1;

/// Spend windows in seconds: minute, hour, day, 30 days.
pub const WINDOWS: [i64; 4] = [60, 3_600, 86_400, 2_592_000];
pub const WINDOW_NAMES: [&str; 4] = ["minute", "hour", "day", "month"];
/// 30 days: older charges leave every window.
pub const MONTH: i64 = 2_592_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Meta {
    pub schema_version: i64,
    pub min_reader_version: i64,
    /// Next `questions.seq`, never reused.
    pub next_seq: i64,
    /// `edges[w]`: rows with `at > edges[w]` are in window `w`.
    pub edges: [i64; 4],
    /// `sums[w]` = Σ nusd of the rows with `at > edges[w]`.
    pub sums: [i64; 4],
    /// Raw `inflight` text: `"seq:lease_until seq:lease_until"`.
    pub inflight: String,
    pub calls_day: i64,
    pub calls_today: i64,
    pub ip_count: i64,
    pub ips_pruned_at: i64,
    pub cmd_next_seq: i64,
    pub cmd_day: i64,
    pub cmd_today: i64,
    pub cmd_last_at: i64,
    pub text_bytes: i64,
    pub text_dropped: i64,
    pub maintained_at: i64,
    pub import_closed: i64,
}

impl Meta {
    /// A new ledger: nothing spent, first seqs 1.
    pub fn fresh(now: i64) -> Meta {
        Meta {
            schema_version: SCHEMA_VERSION,
            min_reader_version: READER_VERSION,
            next_seq: 1,
            edges: WINDOWS.map(|w| now - w),
            sums: [0; 4],
            inflight: String::new(),
            calls_day: -1,
            calls_today: 0,
            ip_count: 0,
            ips_pruned_at: 0,
            cmd_next_seq: 1,
            cmd_day: -1,
            cmd_today: 0,
            cmd_last_at: 0,
            text_bytes: 0,
            text_dropped: 0,
            maintained_at: now,
            import_closed: 0,
        }
    }

    /// Decodes a row of `META_SELECT`; `None` when a column is missing or has the wrong type.
    pub fn from_row(row: &[SqlValue]) -> Option<Meta> {
        let int = |i: usize| col_i64(row, i);
        Some(Meta {
            schema_version: int(0)?,
            min_reader_version: int(1)?,
            next_seq: int(2)?,
            edges: [int(3)?, int(5)?, int(7)?, int(9)?],
            sums: [int(4)?, int(6)?, int(8)?, int(10)?],
            inflight: row.get(11)?.as_str()?.to_string(),
            calls_day: int(12)?,
            calls_today: int(13)?,
            ip_count: int(14)?,
            ips_pruned_at: int(15)?,
            cmd_next_seq: int(16)?,
            cmd_day: int(17)?,
            cmd_today: int(18)?,
            cmd_last_at: int(19)?,
            text_bytes: int(20)?,
            text_dropped: int(21)?,
            maintained_at: int(22)?,
            import_closed: int(23)?,
        })
    }

    /// Parameters of `META_UPSERT`, in its column order.
    pub fn to_params(&self) -> Vec<SqlValue> {
        let [e0, e1, e2, e3] = self.edges;
        let [s0, s1, s2, s3] = self.sums;
        vec![
            self.schema_version.into(),
            self.min_reader_version.into(),
            self.next_seq.into(),
            e0.into(),
            s0.into(),
            e1.into(),
            s1.into(),
            e2.into(),
            s2.into(),
            e3.into(),
            s3.into(),
            self.inflight.clone().into(),
            self.calls_day.into(),
            self.calls_today.into(),
            self.ip_count.into(),
            self.ips_pruned_at.into(),
            self.cmd_next_seq.into(),
            self.cmd_day.into(),
            self.cmd_today.into(),
            self.cmd_last_at.into(),
            self.text_bytes.into(),
            self.text_dropped.into(),
            self.maintained_at.into(),
            self.import_closed.into(),
        ]
    }

    /// The first reason this row cannot be trusted, if any (design 3.3). `schema_version` is
    /// informational: a newer schema with a readable `min_reader_version` is accepted.
    pub fn problem(&self) -> Option<&'static str> {
        if self.min_reader_version > READER_VERSION || self.min_reader_version < 1 {
            return Some("reader_version");
        }
        if self.next_seq < 1 || self.cmd_next_seq < 1 {
            return Some("seq");
        }
        if self.sums.iter().any(|s| *s < 0) {
            return Some("negative_sum");
        }
        let counters = [
            self.calls_today,
            self.ip_count,
            self.cmd_today,
            self.text_bytes,
            self.text_dropped,
        ];
        if counters.iter().any(|c| *c < 0) {
            return Some("negative_counter");
        }
        if parse_inflight(&self.inflight).is_none() {
            return Some("inflight");
        }
        None
    }
}

/// `"seq:lease_until seq:lease_until"` → pairs; `None` when a token does not parse.
pub fn parse_inflight(text: &str) -> Option<Vec<(i64, i64)>> {
    text.split_whitespace()
        .map(|token| {
            let (seq, lease) = token.split_once(':')?;
            Some((seq.parse().ok()?, lease.parse().ok()?))
        })
        .collect()
}

pub fn format_inflight(entries: &[(i64, i64)]) -> String {
    entries
        .iter()
        .map(|(seq, lease)| format!("{seq}:{lease}"))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inflight_codec() {
        assert_eq!(parse_inflight(""), Some(vec![]));
        assert_eq!(
            parse_inflight("4:120 5:121"),
            Some(vec![(4, 120), (5, 121)])
        );
        assert_eq!(parse_inflight("4:120 x"), None);
        assert_eq!(parse_inflight("4-120"), None);
        assert_eq!(format_inflight(&[(4, 120), (5, 121)]), "4:120 5:121");
        assert_eq!(format_inflight(&[]), "");
    }

    #[test]
    fn row_round_trip_and_validation() {
        let mut m = Meta::fresh(1_000_000);
        m.inflight = "3:1000020".into();
        let row: Vec<SqlValue> = m.to_params();
        assert_eq!(row.len(), 24);
        assert_eq!(Meta::from_row(&row), Some(m.clone()));
        assert_eq!(m.problem(), None);
        let mut short = row.clone();
        short.pop();
        assert_eq!(Meta::from_row(&short), None);
        let mut bad = m.clone();
        bad.min_reader_version = 2;
        assert_eq!(bad.problem(), Some("reader_version"));
        let mut newer = m.clone();
        newer.schema_version = 5;
        assert_eq!(newer.problem(), None);
        let mut bad = m.clone();
        bad.sums[2] = -1;
        assert_eq!(bad.problem(), Some("negative_sum"));
        let mut bad = m.clone();
        bad.next_seq = 0;
        assert_eq!(bad.problem(), Some("seq"));
        let mut bad = m.clone();
        bad.text_bytes = -3;
        assert_eq!(bad.problem(), Some("negative_counter"));
        let mut bad = m;
        bad.inflight = "garbage".into();
        assert_eq!(bad.problem(), Some("inflight"));
    }
}
