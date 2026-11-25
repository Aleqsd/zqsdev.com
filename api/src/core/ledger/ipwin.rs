//! Per-IP admission windows (spec §7.1): 4 per second, 8 per minute, 60 per hour and 120 per day
//! for each IPv4 address or IPv6 /64, and at most 4,096 keys per 24 h.

/// `(window seconds, limit)`: a call is refused when `limit` admitted calls already happened
/// with `now − t < window`.
pub const IP_WINDOWS: [(i64, usize); 4] = [(1, 4), (60, 8), (3_600, 60), (86_400, 120)];
/// Distinct keys kept for 24 h.
pub const IP_CAP: i64 = 4_096;
/// Hits older than this are forgotten.
pub const IP_HISTORY: i64 = 86_400;
/// At the key cap, stale keys are pruned at most this often.
pub const IP_PRUNE_EVERY: i64 = 60;
/// Hits kept per key (the daily limit).
pub const MAX_HITS: usize = 120;

/// Decodes the space-separated hits, keeping those of the last 24 h in ascending order.
/// Unreadable tokens are skipped: a damaged row only forgets history.
pub fn parse_hits(text: &str, now: i64) -> Vec<i64> {
    let mut hits: Vec<i64> = text
        .split_whitespace()
        .filter_map(|t| t.parse::<i64>().ok())
        .filter(|t| now.saturating_sub(*t) < IP_HISTORY)
        .collect();
    hits.sort_unstable();
    let excess = hits.len().saturating_sub(MAX_HITS);
    hits.drain(..excess);
    hits
}

pub fn format_hits(hits: &[i64]) -> String {
    hits.iter()
        .map(i64::to_string)
        .collect::<Vec<_>>()
        .join(" ")
}

/// The first window (in seconds) that is full at `now`, if any.
pub fn full_window(hits: &[i64], now: i64) -> Option<i64> {
    IP_WINDOWS
        .iter()
        .find(|(window, limit)| {
            hits.iter()
                .filter(|t| now.saturating_sub(**t) < *window)
                .count()
                >= *limit
        })
        .map(|(window, _)| *window)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ported from `budget.rs` (window part): 4 hits in one second pass, the 5th fails, 2 s later
    /// it passes again.
    #[test]
    fn ip_limits_expire() {
        let mut hits = Vec::new();
        for _ in 0..4 {
            assert_eq!(full_window(&hits, 100), None);
            hits.push(100);
        }
        assert_eq!(full_window(&hits, 100), Some(1));
        assert_eq!(full_window(&hits, 102), None);
        // History older than 24 h is forgotten.
        assert_eq!(
            parse_hits(&format_hits(&hits), 100 + 86_400),
            Vec::<i64>::new()
        );
        assert_eq!(parse_hits(&format_hits(&hits), 100 + 86_399), hits);
    }

    #[test]
    fn every_window() {
        let minute: Vec<i64> = (0..8).map(|i| 1_000 + i * 2).collect();
        assert_eq!(full_window(&minute, 1_020), Some(60));
        assert_eq!(full_window(&minute, 1_060), None);
        let hour: Vec<i64> = (0..60).map(|i| 10_000 + i * 61).collect();
        assert_eq!(full_window(&hour, 13_599), Some(3_600));
        assert_eq!(full_window(&hour, 13_600), None);
        let day: Vec<i64> = (0..120).map(|i| 100_000 + i * 700).collect();
        assert_eq!(full_window(&day, 186_000), Some(86_400));
        assert_eq!(full_window(&day, 186_400), None);
    }

    #[test]
    fn codec_tolerates_damage() {
        assert_eq!(parse_hits("5 x 3  9", 10), vec![3, 5, 9]);
        let many = format_hits(&(0..130).collect::<Vec<_>>());
        let kept = parse_hits(&many, 129);
        assert_eq!(kept.len(), MAX_HITS);
        assert_eq!(kept.first(), Some(&10));
    }
}
