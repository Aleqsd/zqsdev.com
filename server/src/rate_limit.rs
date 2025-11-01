use axum::http::StatusCode;
use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

const PER_IP_BURST_MAX: usize = 4;
const PER_IP_MINUTE_MAX: usize = 8;
const PER_IP_HOUR_MAX: usize = 60;
const PER_IP_DAY_MAX: usize = 120;

const BURST: Duration = Duration::from_secs(1);
const MINUTE: Duration = Duration::from_secs(60);
const HOUR: Duration = Duration::from_secs(60 * 60);
const DAY: Duration = Duration::from_secs(60 * 60 * 24);
const MONTH: Duration = Duration::from_secs(60 * 60 * 24 * 30);

pub struct RateLimiter {
    minute_cost: CostWindow,
    hour_cost: CostWindow,
    day_cost: CostWindow,
    month_cost: CostWindow,
    per_ip: HashMap<String, IpWindows>,
}

#[derive(Debug, Clone)]
pub struct UsageSnapshot {
    pub minute_spend: f64,
    pub hour_spend: f64,
    pub day_spend: f64,
    pub month_spend: f64,
    pub ip_burst: usize,
    pub ip_minute: usize,
    pub ip_hour: usize,
    pub ip_day: usize,
}

struct CostWindow {
    duration: Duration,
    budget_eur: f64,
    entries: VecDeque<(Instant, f64)>,
    total: f64,
}

struct IpWindows {
    burst: CountWindow,
    minute: CountWindow,
    hour: CountWindow,
    day: CountWindow,
}

struct CountWindow {
    duration: Duration,
    limit: usize,
    entries: VecDeque<Instant>,
}

#[derive(Debug)]
pub enum RateLimitError {
    PerIpBurst,
    PerIpMinute,
    PerIpHour,
    PerIpDay,
    MinuteBudget,
    HourBudget,
    DayBudget,
    MonthBudget,
}

impl RateLimiter {
    pub fn new(minute_budget: f64, hour_budget: f64, day_budget: f64, month_budget: f64) -> Self {
        Self {
            minute_cost: CostWindow::new(MINUTE, minute_budget),
            hour_cost: CostWindow::new(HOUR, hour_budget),
            day_cost: CostWindow::new(DAY, day_budget),
            month_cost: CostWindow::new(MONTH, month_budget),
            per_ip: HashMap::new(),
        }
    }

    pub fn check_and_record(&mut self, ip: &str, cost: f64) -> Result<(), RateLimitError> {
        let now = Instant::now();

        if cost > self.minute_cost.budget_eur {
            return Err(RateLimitError::MinuteBudget);
        }
        if cost > self.hour_cost.budget_eur {
            return Err(RateLimitError::HourBudget);
        }
        if cost > self.day_cost.budget_eur {
            return Err(RateLimitError::DayBudget);
        }
        if cost > self.month_cost.budget_eur {
            return Err(RateLimitError::MonthBudget);
        }

        self.minute_cost.prune(now);
        self.hour_cost.prune(now);
        self.day_cost.prune(now);
        self.month_cost.prune(now);

        let ip_windows = self
            .per_ip
            .entry(ip.to_string())
            .or_insert_with(IpWindows::new);
        if ip_windows.burst.would_exceed(now) {
            return Err(RateLimitError::PerIpBurst);
        }
        if ip_windows.minute.would_exceed(now) {
            return Err(RateLimitError::PerIpMinute);
        }
        if ip_windows.hour.would_exceed(now) {
            return Err(RateLimitError::PerIpHour);
        }
        if ip_windows.day.would_exceed(now) {
            return Err(RateLimitError::PerIpDay);
        }

        if self.minute_cost.would_exceed(cost) {
            return Err(RateLimitError::MinuteBudget);
        }
        if self.hour_cost.would_exceed(cost) {
            return Err(RateLimitError::HourBudget);
        }
        if self.day_cost.would_exceed(cost) {
            return Err(RateLimitError::DayBudget);
        }
        if self.month_cost.would_exceed(cost) {
            return Err(RateLimitError::MonthBudget);
        }

        self.minute_cost.record(now, cost);
        self.hour_cost.record(now, cost);
        self.day_cost.record(now, cost);
        self.month_cost.record(now, cost);
        ip_windows.burst.record(now);
        ip_windows.minute.record(now);
        ip_windows.hour.record(now);
        ip_windows.day.record(now);

        Ok(())
    }

    pub fn usage_snapshot(&self, ip: &str) -> UsageSnapshot {
        let ip_windows = self.per_ip.get(ip);
        UsageSnapshot {
            minute_spend: self.minute_cost.total,
            hour_spend: self.hour_cost.total,
            day_spend: self.day_cost.total,
            month_spend: self.month_cost.total,
            ip_burst: ip_windows.map(|w| w.burst.entries.len()).unwrap_or(0),
            ip_minute: ip_windows.map(|w| w.minute.entries.len()).unwrap_or(0),
            ip_hour: ip_windows.map(|w| w.hour.entries.len()).unwrap_or(0),
            ip_day: ip_windows.map(|w| w.day.entries.len()).unwrap_or(0),
        }
    }

    pub fn record_cost_if_within(&mut self, cost: f64) -> Result<(), RateLimitError> {
        if cost <= 0.0 {
            return Ok(());
        }

        let now = Instant::now();
        self.minute_cost.prune(now);
        self.hour_cost.prune(now);
        self.day_cost.prune(now);
        self.month_cost.prune(now);

        if self.minute_cost.would_exceed(cost) {
            return Err(RateLimitError::MinuteBudget);
        }
        if self.hour_cost.would_exceed(cost) {
            return Err(RateLimitError::HourBudget);
        }
        if self.day_cost.would_exceed(cost) {
            return Err(RateLimitError::DayBudget);
        }
        if self.month_cost.would_exceed(cost) {
            return Err(RateLimitError::MonthBudget);
        }

        self.minute_cost.record(now, cost);
        self.hour_cost.record(now, cost);
        self.day_cost.record(now, cost);
        self.month_cost.record(now, cost);
        Ok(())
    }
}

impl RateLimitError {
    pub fn describe(&self) -> (StatusCode, &'static str, &'static str) {
        match self {
            RateLimitError::PerIpBurst => (
                StatusCode::TOO_MANY_REQUESTS,
                "per_ip_burst",
                "per-second request limit",
            ),
            RateLimitError::PerIpMinute => (
                StatusCode::TOO_MANY_REQUESTS,
                "per_ip_minute",
                "per-minute request limit",
            ),
            RateLimitError::PerIpHour => (
                StatusCode::TOO_MANY_REQUESTS,
                "per_ip_hour",
                "per-hour request limit",
            ),
            RateLimitError::PerIpDay => (
                StatusCode::TOO_MANY_REQUESTS,
                "per_ip_day",
                "per-day request limit",
            ),
            RateLimitError::MinuteBudget => (
                StatusCode::TOO_MANY_REQUESTS,
                "minute_budget",
                "per-minute budget",
            ),
            RateLimitError::HourBudget => (
                StatusCode::TOO_MANY_REQUESTS,
                "hour_budget",
                "per-hour budget",
            ),
            RateLimitError::DayBudget => (
                StatusCode::TOO_MANY_REQUESTS,
                "day_budget",
                "per-day budget",
            ),
            RateLimitError::MonthBudget => (
                StatusCode::TOO_MANY_REQUESTS,
                "month_budget",
                "monthly budget",
            ),
        }
    }
}

impl CostWindow {
    fn new(duration: Duration, budget_eur: f64) -> Self {
        Self {
            duration,
            budget_eur,
            entries: VecDeque::new(),
            total: 0.0,
        }
    }

    fn prune(&mut self, now: Instant) {
        while let Some((timestamp, _)) = self.entries.front() {
            if now.duration_since(*timestamp) > self.duration {
                if let Some((_, amount)) = self.entries.pop_front() {
                    self.total -= amount;
                }
            } else {
                break;
            }
        }
        if self.total < 0.0 {
            self.total = 0.0;
        }
    }

    fn would_exceed(&self, cost: f64) -> bool {
        self.total + cost > self.budget_eur + f64::EPSILON
    }

    fn record(&mut self, now: Instant, cost: f64) {
        self.entries.push_back((now, cost));
        self.total += cost;
    }
}

impl IpWindows {
    fn new() -> Self {
        Self {
            burst: CountWindow::new(BURST, PER_IP_BURST_MAX),
            minute: CountWindow::new(MINUTE, PER_IP_MINUTE_MAX),
            hour: CountWindow::new(HOUR, PER_IP_HOUR_MAX),
            day: CountWindow::new(DAY, PER_IP_DAY_MAX),
        }
    }
}

impl CountWindow {
    fn new(duration: Duration, limit: usize) -> Self {
        Self {
            duration,
            limit,
            entries: VecDeque::new(),
        }
    }

    fn prune(&mut self, now: Instant) {
        while let Some(timestamp) = self.entries.front().copied() {
            if now.duration_since(timestamp) > self.duration {
                self.entries.pop_front();
            } else {
                break;
            }
        }
    }

    fn would_exceed(&mut self, now: Instant) -> bool {
        self.prune(now);
        self.entries.len() >= self.limit
    }

    fn record(&mut self, now: Instant) {
        self.entries.push_back(now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn per_ip_limits_are_enforced() {
        let mut limiter = RateLimiter::new(1.0, 2.0, 5.0, 10.0);
        let ip = "127.0.0.1";
        for _ in 0..PER_IP_BURST_MAX {
            limiter.check_and_record(ip, 0.01).unwrap();
        }
        assert!(matches!(
            limiter.check_and_record(ip, 0.01).unwrap_err(),
            RateLimitError::PerIpBurst
        ));

        std::thread::sleep(BURST + std::time::Duration::from_millis(10));

        let mut limiter = RateLimiter::new(1.0, 2.0, 5.0, 10.0);
        for attempt in 0..PER_IP_MINUTE_MAX {
            limiter.check_and_record(ip, 0.01).unwrap();
            if attempt + 1 < PER_IP_MINUTE_MAX {
                std::thread::sleep(BURST + std::time::Duration::from_millis(10));
            }
        }
        assert!(matches!(
            limiter.check_and_record(ip, 0.01).unwrap_err(),
            RateLimitError::PerIpMinute
        ));
    }

    #[test]
    fn minute_budget_blocks_excess_cost() {
        let mut limiter = RateLimiter::new(0.05, 1.0, 1.0, 1.0);
        let ip = "192.168.0.5";
        assert!(limiter.check_and_record(ip, 0.02).is_ok());
        assert!(limiter.check_and_record(ip, 0.02).is_ok());
        std::thread::sleep(BURST + std::time::Duration::from_millis(10));
        assert!(matches!(
            limiter.check_and_record(ip, 0.02).unwrap_err(),
            RateLimitError::MinuteBudget
        ));
    }

    #[test]
    fn usage_snapshot_reports_recent_activity() {
        let mut limiter = RateLimiter::new(0.5, 2.0, 5.0, 10.0);
        let ip = "203.0.113.4";
        limiter.check_and_record(ip, 0.1).unwrap();
        let snapshot = limiter.usage_snapshot(ip);
        assert!(snapshot.minute_spend >= 0.1 - f64::EPSILON);
        assert_eq!(snapshot.ip_burst, 1);
        assert_eq!(snapshot.ip_minute, 1);
    }
}
