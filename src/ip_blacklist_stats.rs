use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::Instant;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IpBlacklistSummary {
    pub blocked: u64,
    pub unique_ips: usize,
    pub window_secs: u64,
    pub top_ips: Vec<(String, u64)>,
    pub top_paths: Vec<(String, u64)>,
}

#[derive(Debug)]
pub struct IpBlacklistStats {
    inner: Mutex<IpBlacklistStatsInner>,
    top_limit: usize,
}

#[derive(Debug)]
struct IpBlacklistStatsInner {
    window_started: Instant,
    blocked: u64,
    by_ip: HashMap<String, u64>,
    by_path: HashMap<String, u64>,
}

impl IpBlacklistStats {
    pub fn new() -> Self {
        Self::with_top_limit(10)
    }

    fn with_top_limit(top_limit: usize) -> Self {
        Self {
            inner: Mutex::new(IpBlacklistStatsInner {
                window_started: Instant::now(),
                blocked: 0,
                by_ip: HashMap::new(),
                by_path: HashMap::new(),
            }),
            top_limit,
        }
    }

    pub fn record(&self, ip: IpAddr, path: &str) {
        let mut inner = self.inner.lock().expect("ip blacklist stats lock poisoned");
        inner.blocked += 1;
        *inner.by_ip.entry(ip.to_string()).or_default() += 1;
        *inner.by_path.entry(path.to_string()).or_default() += 1;
    }

    pub fn flush(&self) -> Option<IpBlacklistSummary> {
        let mut inner = self.inner.lock().expect("ip blacklist stats lock poisoned");
        if inner.blocked == 0 {
            inner.window_started = Instant::now();
            return None;
        }

        let summary = IpBlacklistSummary {
            blocked: inner.blocked,
            unique_ips: inner.by_ip.len(),
            window_secs: inner.window_started.elapsed().as_secs().max(1),
            top_ips: top_counts(&inner.by_ip, self.top_limit),
            top_paths: top_counts(&inner.by_path, self.top_limit),
        };

        *inner = IpBlacklistStatsInner {
            window_started: Instant::now(),
            blocked: 0,
            by_ip: HashMap::new(),
            by_path: HashMap::new(),
        };

        Some(summary)
    }
}

impl Default for IpBlacklistStats {
    fn default() -> Self {
        Self::new()
    }
}

fn top_counts(values: &HashMap<String, u64>, limit: usize) -> Vec<(String, u64)> {
    let mut counts = values
        .iter()
        .map(|(key, count)| (key.clone(), *count))
        .collect::<Vec<_>>();
    counts.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
    counts.truncate(limit);
    counts
}

pub fn format_top_counts(values: &[(String, u64)]) -> String {
    values
        .iter()
        .map(|(key, count)| format!("{key}:{count}"))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_and_flushes_blacklist_summary() {
        let stats = IpBlacklistStats::with_top_limit(2);
        stats.record("203.0.113.10".parse().unwrap(), "/");
        stats.record("203.0.113.10".parse().unwrap(), "/v1/chat/completions");
        stats.record("198.51.100.7".parse().unwrap(), "/");
        stats.record("192.0.2.8".parse().unwrap(), "/");

        let summary = stats.flush().expect("summary");

        assert_eq!(summary.blocked, 4);
        assert_eq!(summary.unique_ips, 3);
        assert_eq!(
            summary.top_ips,
            vec![
                ("203.0.113.10".to_string(), 2),
                ("192.0.2.8".to_string(), 1),
            ]
        );
        assert_eq!(
            summary.top_paths,
            vec![
                ("/".to_string(), 3),
                ("/v1/chat/completions".to_string(), 1),
            ]
        );
        assert!(stats.flush().is_none());
    }

    #[test]
    fn empty_flush_produces_no_summary() {
        let stats = IpBlacklistStats::new();
        assert!(stats.flush().is_none());
    }

    #[test]
    fn formats_top_counts_for_logs() {
        assert_eq!(
            format_top_counts(&[("203.0.113.10".to_string(), 3), ("/".to_string(), 2)]),
            "203.0.113.10:3, /:2"
        );
    }

    #[test]
    fn one_second_minimum_window_avoids_zero_length_summary() {
        let stats = IpBlacklistStats::new();
        stats.record("203.0.113.10".parse().unwrap(), "/");
        let summary = stats.flush().expect("summary");
        assert_eq!(summary.window_secs, 1);
    }
}
