//! Admission happens before provider requests. All amounts are USD.
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, VecDeque},
    path::PathBuf,
};
const MONTH: i64 = 30 * 86400;
#[derive(Clone, Serialize, Deserialize)]
struct Charge {
    id: String,
    at: i64,
    usd: f64,
}
pub struct Budget {
    path: PathBuf,
    charges: Vec<Charge>,
    ips: HashMap<String, VecDeque<i64>>,
    limits: [f64; 4],
}
impl Budget {
    pub fn open(path: PathBuf, limits: [f64; 4]) -> Result<Self> {
        if limits.iter().any(|x| !x.is_finite() || *x < 0.0) {
            bail!("Invalid spending limits");
        }
        let charges: Vec<Charge> = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).context("Invalid budget ledger")?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(e.into()),
        };
        if charges.iter().any(|c| !c.usd.is_finite() || c.usd < 0.0) {
            bail!("Invalid ledger cost");
        }
        let instance = Self {
            path,
            charges,
            ips: HashMap::new(),
            limits,
        };
        instance.persist()?;
        Ok(instance)
    }
    fn persist(&self) -> Result<()> {
        if let Some(p) = self.path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(p)?;
        }
        let tmp = self.path.with_extension("tmp");
        let mut file = std::fs::File::create(&tmp)?;
        use std::io::Write;
        file.write_all(&serde_json::to_vec(&self.charges)?)?;
        file.sync_all()?;
        std::fs::rename(tmp, &self.path)?;
        #[cfg(unix)]
        if let Some(p) = self.path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::File::open(p)?.sync_all()?;
        }
        Ok(())
    }
    pub fn admit_ip(&mut self, ip: &str, now: i64) -> Result<()> {
        self.ips.retain(|_, times| {
            while times.front().is_some_and(|t| now - *t >= 86400) {
                times.pop_front();
            }
            !times.is_empty()
        });
        if !self.ips.contains_key(ip) && self.ips.len() >= 4096 {
            bail!("traffic_limit");
        }
        let times = self.ips.entry(ip.to_string()).or_default();
        for (window, limit) in [(1, 4), (60, 8), (3600, 60), (86400, 120)] {
            if times.iter().filter(|t| now - **t < window).count() >= limit {
                bail!("request_limit");
            }
        }
        times.push_back(now);
        Ok(())
    }
    pub fn reserve(&mut self, id: &str, usd: f64, now: i64) -> Result<()> {
        if !usd.is_finite() || usd <= 0.0 {
            bail!("Invalid reservation");
        }
        self.charges.retain(|c| now - c.at < MONTH);
        for (window, limit) in [60, 3600, 86400, MONTH].into_iter().zip(self.limits) {
            let spent: f64 = self
                .charges
                .iter()
                .filter(|c| now - c.at < window)
                .map(|c| c.usd)
                .sum();
            if spent + usd > limit {
                bail!("spending_limit");
            }
        }
        self.charges.push(Charge {
            id: id.to_string(),
            at: now,
            usd,
        });
        // Persist before sending. On failure the request is denied.
        self.persist()
    }
    pub fn settle(&mut self, id: &str, actual_usd: f64) -> Result<()> {
        if !actual_usd.is_finite() || actual_usd < 0.0 {
            bail!("Invalid usage cost");
        }
        let charge = self
            .charges
            .iter_mut()
            .find(|c| c.id == id)
            .context("Unknown reservation")?;
        charge.usd = actual_usd;
        self.persist()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn ledger() -> Budget {
        Budget::open(
            std::env::temp_dir().join(format!("zqs-budget-{}.json", uuid::Uuid::new_v4())),
            [0.5, 2.0, 2.0, 10.0],
        )
        .unwrap()
    }
    #[test]
    fn settlement_replaces_reservation_and_survives_restart() {
        let mut l = ledger();
        l.reserve("one", 0.1, 100).unwrap();
        l.settle("one", 0.02).unwrap();
        let restored = Budget::open(l.path.clone(), l.limits).unwrap();
        assert_eq!(restored.charges.len(), 1);
        assert_eq!(restored.charges[0].usd, 0.02);
        std::fs::remove_file(l.path).unwrap();
    }
    #[test]
    fn crash_retains_reservation_and_blocks_overspend() {
        let mut l = ledger();
        l.reserve("one", 0.49, 100).unwrap();
        let mut restored = Budget::open(l.path.clone(), l.limits).unwrap();
        assert!(restored.reserve("two", 0.02, 101).is_err());
        restored.reserve("later", 0.02, 161).unwrap();
        std::fs::remove_file(l.path).unwrap();
    }
    #[test]
    fn ip_limits_expire() {
        let mut l = ledger();
        for _ in 0..4 {
            l.admit_ip("a", 100).unwrap();
        }
        assert!(l.admit_ip("a", 100).is_err());
        l.admit_ip("a", 102).unwrap();
        l.admit_ip("b", 100000).unwrap();
        assert_eq!(l.ips.len(), 1);
        std::fs::remove_file(l.path).unwrap();
    }
    #[test]
    fn corrupted_ledger_fails_closed() {
        let l = ledger();
        std::fs::write(&l.path, "invalid").unwrap();
        assert!(Budget::open(l.path.clone(), l.limits).is_err());
        std::fs::remove_file(l.path).unwrap();
    }
}
