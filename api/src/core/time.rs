//! UTC calendar formatting without chrono (civil-from-days, H. Hinnant), exact for every `i64`
//! second count the API can meet.

/// Seconds per day.
pub const DAY: i64 = 86_400;

/// UTC day number (days since 1970-01-01) of a unix timestamp.
pub fn day_number(secs: i64) -> i64 {
    secs.div_euclid(DAY)
}

/// `YYYY-MM-DD` of a unix timestamp, as `chrono::Utc::now().date_naive()` displays it.
pub fn utc_date(secs: i64) -> String {
    let (y, m, d) = civil_from_days(day_number(secs));
    format!("{y:04}-{m:02}-{d:02}")
}

/// RFC 3339 UTC with seconds and a `Z` suffix (`to_rfc3339_opts(SecondsFormat::Secs, true)`).
pub fn rfc3339(secs: i64) -> String {
    let (y, m, d) = civil_from_days(day_number(secs));
    let sod = secs.rem_euclid(DAY);
    let (hh, mm, ss) = (sod / 3_600, (sod % 3_600) / 60, sod % 60);
    format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}Z")
}

/// Proleptic Gregorian (year, month, day) of a day count relative to 1970-01-01.
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097); // [0, 146096]
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc3339_vectors() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(rfc3339(1_764_000_000), "2025-11-24T16:00:00Z");
        assert_eq!(rfc3339(1_795_478_400), "2026-11-24T00:00:00Z");
        assert_eq!(rfc3339(1_795_564_799), "2026-11-24T23:59:59Z");
        assert_eq!(rfc3339(-1), "1969-12-31T23:59:59Z");
    }

    #[test]
    fn utc_date_and_day_number() {
        assert_eq!(utc_date(1_795_564_799), "2026-11-24");
        assert_eq!(utc_date(1_795_564_800), "2026-11-25");
        assert_eq!(utc_date(951_868_799), "2000-02-29");
        assert_eq!(utc_date(4_107_542_400), "2100-03-01");
        assert_eq!(day_number(1_795_478_400), 20_781);
        assert_eq!(day_number(-1), -1);
        // No panic at the extremes.
        let _ = rfc3339(i64::MAX);
        let _ = rfc3339(i64::MIN);
    }
}
