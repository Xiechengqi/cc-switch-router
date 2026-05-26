use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::config::Config;

use super::models::{
    DiskInfo, DiskUsage, HostMetricsInfo, HostMetricsStatus, NetworkMetricsStatus,
    ProcessMetricsStatus,
};

#[derive(Debug, Clone, Copy)]
struct CpuSample {
    idle: u64,
    total: u64,
}

#[derive(Debug, Clone, Copy)]
struct NetworkSample {
    rx_bytes: u64,
    tx_bytes: u64,
    timestamp: i64,
}

#[derive(Debug, Default)]
pub struct HostSampler {
    cpu: Option<CpuSample>,
    network: Option<NetworkSample>,
}

impl HostSampler {
    pub fn sample(&mut self, config: &Config, metrics_db_path: &Path) -> HostMetricsStatus {
        let timestamp = chrono::Utc::now().timestamp();
        let cpu_percent = read_cpu_sample().and_then(|next| {
            let percent = self.cpu.and_then(|prev| cpu_percent(prev, next));
            self.cpu = Some(next);
            percent
        });
        let mem = read_meminfo();
        let network_raw = read_network_totals().map(|(rx_bytes, tx_bytes)| NetworkSample {
            rx_bytes,
            tx_bytes,
            timestamp,
        });
        let (rx_bytes_per_sec, tx_bytes_per_sec) = match (self.network, network_raw) {
            (Some(prev), Some(next)) if next.timestamp > prev.timestamp => {
                let secs = (next.timestamp - prev.timestamp) as f64;
                self.network = Some(next);
                (
                    Some(next.rx_bytes.saturating_sub(prev.rx_bytes) as f64 / secs),
                    Some(next.tx_bytes.saturating_sub(prev.tx_bytes) as f64 / secs),
                )
            }
            (_, Some(next)) => {
                self.network = Some(next);
                (None, None)
            }
            _ => (None, None),
        };
        let (tcp_established, tcp_time_wait) = read_tcp_sockstat();
        let (load_1, load_5, load_15) = read_loadavg();

        HostMetricsStatus {
            timestamp,
            uptime_secs: read_uptime_secs(),
            cpu_percent,
            load_1,
            load_5,
            load_15,
            memory_used_bytes: mem
                .as_ref()
                .and_then(|m| m.mem_total.checked_sub(m.mem_available)),
            memory_total_bytes: mem.as_ref().map(|m| m.mem_total),
            memory_available_bytes: mem.as_ref().map(|m| m.mem_available),
            swap_used_bytes: mem
                .as_ref()
                .and_then(|m| m.swap_total.checked_sub(m.swap_free)),
            swap_total_bytes: mem.as_ref().map(|m| m.swap_total),
            disks: read_disk_usage(config, metrics_db_path),
            network: NetworkMetricsStatus {
                rx_bytes_per_sec,
                tx_bytes_per_sec,
                tcp_established,
                tcp_time_wait,
            },
            process: read_process_status(),
        }
    }
}

pub fn host_info(config: &Config, metrics_db_path: &Path) -> HostMetricsInfo {
    HostMetricsInfo {
        hostname: read_hostname(),
        os_name: read_os_release_value("PRETTY_NAME").or_else(|| read_os_release_value("NAME")),
        os_version: read_os_release_value("VERSION"),
        kernel_version: read_command_stdout("uname", &["-r"]),
        arch: std::env::consts::ARCH.to_string(),
        cpu_brand: read_cpu_brand(),
        cpu_cores: std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1),
        memory_total_bytes: read_meminfo().map(|m| m.mem_total),
        disks: read_disk_usage(config, metrics_db_path)
            .into_iter()
            .map(|disk| DiskInfo {
                name: disk.label,
                mount_point: disk.mount_point,
                total_bytes: disk.total_bytes,
            })
            .collect(),
    }
}

#[derive(Debug, Clone)]
struct MemInfo {
    mem_total: u64,
    mem_available: u64,
    swap_total: u64,
    swap_free: u64,
}

fn read_cpu_sample() -> Option<CpuSample> {
    let content = std::fs::read_to_string("/proc/stat").ok()?;
    let line = content.lines().next()?;
    let values = line
        .split_whitespace()
        .skip(1)
        .filter_map(|value| value.parse::<u64>().ok())
        .collect::<Vec<_>>();
    if values.len() < 4 {
        return None;
    }
    let idle = values.get(3).copied().unwrap_or(0) + values.get(4).copied().unwrap_or(0);
    let total = values.iter().copied().sum();
    Some(CpuSample { idle, total })
}

fn cpu_percent(prev: CpuSample, next: CpuSample) -> Option<f64> {
    let total_delta = next.total.checked_sub(prev.total)?;
    let idle_delta = next.idle.checked_sub(prev.idle)?;
    if total_delta == 0 {
        return None;
    }
    Some(
        ((total_delta.saturating_sub(idle_delta)) as f64 * 100.0 / total_delta as f64)
            .clamp(0.0, 100.0),
    )
}

fn read_meminfo() -> Option<MemInfo> {
    let content = std::fs::read_to_string("/proc/meminfo").ok()?;
    let mut values = HashMap::new();
    for line in content.lines() {
        let Some((key, rest)) = line.split_once(':') else {
            continue;
        };
        let kb = rest
            .split_whitespace()
            .next()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(0);
        values.insert(key.to_string(), kb.saturating_mul(1024));
    }
    Some(MemInfo {
        mem_total: *values.get("MemTotal")?,
        mem_available: values
            .get("MemAvailable")
            .copied()
            .or_else(|| values.get("MemFree").copied())?,
        swap_total: values.get("SwapTotal").copied().unwrap_or(0),
        swap_free: values.get("SwapFree").copied().unwrap_or(0),
    })
}

fn read_network_totals() -> Option<(u64, u64)> {
    let content = std::fs::read_to_string("/proc/net/dev").ok()?;
    let mut rx = 0_u64;
    let mut tx = 0_u64;
    for line in content.lines().skip(2) {
        let Some((iface, rest)) = line.split_once(':') else {
            continue;
        };
        if iface.trim() == "lo" {
            continue;
        }
        let values = rest
            .split_whitespace()
            .filter_map(|value| value.parse::<u64>().ok())
            .collect::<Vec<_>>();
        if values.len() >= 16 {
            rx = rx.saturating_add(values[0]);
            tx = tx.saturating_add(values[8]);
        }
    }
    Some((rx, tx))
}

fn read_tcp_sockstat() -> (Option<u64>, Option<u64>) {
    let content = match std::fs::read_to_string("/proc/net/sockstat") {
        Ok(content) => content,
        Err(_) => return (None, None),
    };
    for line in content.lines() {
        if !line.starts_with("TCP:") {
            continue;
        }
        let mut established = None;
        let mut time_wait = None;
        let pieces = line.split_whitespace().collect::<Vec<_>>();
        for pair in pieces.windows(2) {
            match pair[0] {
                "inuse" => established = pair[1].parse().ok(),
                "tw" => time_wait = pair[1].parse().ok(),
                _ => {}
            }
        }
        return (established, time_wait);
    }
    (None, None)
}

fn read_loadavg() -> (Option<f64>, Option<f64>, Option<f64>) {
    let content = match std::fs::read_to_string("/proc/loadavg") {
        Ok(content) => content,
        Err(_) => return (None, None, None),
    };
    let values = content
        .split_whitespace()
        .take(3)
        .map(|value| value.parse::<f64>().ok())
        .collect::<Vec<_>>();
    (
        values.first().copied().flatten(),
        values.get(1).copied().flatten(),
        values.get(2).copied().flatten(),
    )
}

fn read_uptime_secs() -> Option<u64> {
    let content = std::fs::read_to_string("/proc/uptime").ok()?;
    let raw = content.split_whitespace().next()?;
    raw.parse::<f64>().ok().map(|value| value.max(0.0) as u64)
}

fn read_process_status() -> ProcessMetricsStatus {
    let open_fds = std::fs::read_dir("/proc/self/fd")
        .ok()
        .map(|entries| entries.count() as u64);
    let max_fds = read_max_open_files();
    let fd_usage_percent = match (open_fds, max_fds) {
        (Some(open), Some(max)) if max > 0 => Some(open as f64 * 100.0 / max as f64),
        _ => None,
    };
    let (threads, rss_bytes) = read_self_status();
    ProcessMetricsStatus {
        open_fds,
        max_fds,
        fd_usage_percent,
        threads,
        rss_bytes,
        cpu_percent: None,
        uptime_secs: read_process_uptime_secs(),
    }
}

fn read_max_open_files() -> Option<u64> {
    let content = std::fs::read_to_string("/proc/self/limits").ok()?;
    for line in content.lines() {
        if line.starts_with("Max open files") {
            return line
                .split_whitespace()
                .find_map(|value| value.parse::<u64>().ok());
        }
    }
    None
}

fn read_self_status() -> (Option<u64>, Option<u64>) {
    let content = match std::fs::read_to_string("/proc/self/status") {
        Ok(content) => content,
        Err(_) => return (None, None),
    };
    let mut threads = None;
    let mut rss_bytes = None;
    for line in content.lines() {
        if let Some(value) = line.strip_prefix("Threads:") {
            threads = value.trim().parse().ok();
        } else if let Some(value) = line.strip_prefix("VmRSS:") {
            rss_bytes = value
                .split_whitespace()
                .next()
                .and_then(|kb| kb.parse::<u64>().ok())
                .map(|kb| kb.saturating_mul(1024));
        }
    }
    (threads, rss_bytes)
}

fn read_process_uptime_secs() -> Option<u64> {
    let stat = std::fs::read_to_string("/proc/self/stat").ok()?;
    let start_ticks = stat.split_whitespace().nth(21)?.parse::<u64>().ok()?;
    let uptime = read_uptime_secs()?;
    let ticks_per_second = 100_u64;
    Some(uptime.saturating_sub(start_ticks / ticks_per_second))
}

fn read_disk_usage(config: &Config, metrics_db_path: &Path) -> Vec<DiskUsage> {
    let mut paths = vec![
        ("root".to_string(), PathBuf::from("/")),
        ("business db".to_string(), config.db_path.clone()),
        ("metrics db".to_string(), metrics_db_path.to_path_buf()),
        (
            "logs".to_string(),
            PathBuf::from("/tmp/cc-switch-router.log"),
        ),
    ];
    for (_, path) in &mut paths {
        if path.is_file() {
            if let Some(parent) = path.parent() {
                *path = parent.to_path_buf();
            }
        }
    }
    let mut seen_mounts = HashSet::new();
    let mut disks: Vec<DiskUsage> = Vec::new();
    for (label, path) in paths {
        if let Some(mut usage) = df_usage(&path) {
            if !seen_mounts.insert(usage.mount_point.clone()) {
                if let Some(existing) = disks
                    .iter_mut()
                    .find(|disk| disk.mount_point == usage.mount_point)
                {
                    existing.label = format!("{}, {label}", existing.label);
                }
                continue;
            }
            usage.label = label;
            disks.push(usage);
        }
    }
    disks
}

fn df_usage(path: &Path) -> Option<DiskUsage> {
    let output = Command::new("df")
        .arg("-B1")
        .arg("-P")
        .arg(path)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8(output.stdout).ok()?;
    let line = stdout.lines().nth(1)?;
    let parts = line.split_whitespace().collect::<Vec<_>>();
    if parts.len() < 6 {
        return None;
    }
    Some(DiskUsage {
        label: String::new(),
        mount_point: parts[5].to_string(),
        total_bytes: parts[1].parse().ok()?,
        used_bytes: parts[2].parse().ok()?,
    })
}

fn read_hostname() -> Option<String> {
    std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .or_else(|| read_command_stdout("hostname", &[]))
}

fn read_os_release_value(key: &str) -> Option<String> {
    let content = std::fs::read_to_string("/etc/os-release").ok()?;
    for line in content.lines() {
        let Some((line_key, value)) = line.split_once('=') else {
            continue;
        };
        if line_key == key {
            return Some(value.trim_matches('"').to_string());
        }
    }
    None
}

fn read_cpu_brand() -> Option<String> {
    let content = std::fs::read_to_string("/proc/cpuinfo").ok()?;
    for line in content.lines() {
        if let Some(value) = line.strip_prefix("model name") {
            let name = value.trim_start_matches(':').trim();
            if !name.is_empty() {
                return Some(name.to_string());
            }
        }
    }
    None
}

fn read_command_stdout(program: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(program).args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}
