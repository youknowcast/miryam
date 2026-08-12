use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Context;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum CpuLevel {
    Idle,
    Normal,
    High,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum MemLevel {
    Normal,
    High,
}

/// /proc/loadavg の先頭フィールド (load1) を返す
fn parse_loadavg(content: &str) -> anyhow::Result<f64> {
    content
        .split_whitespace()
        .next()
        .context("/proc/loadavg が空です")?
        .parse::<f64>()
        .context("/proc/loadavg の load1 が数値ではありません")
}

/// r = load1 / cores で判定: r < 0.1 → Idle, r >= 0.8 → High, それ以外 Normal
fn cpu_level(load1: f64, cores: u32) -> CpuLevel {
    let ratio = load1 / f64::from(cores.max(1));
    if ratio < 0.1 {
        CpuLevel::Idle
    } else if ratio >= 0.8 {
        CpuLevel::High
    } else {
        CpuLevel::Normal
    }
}

/// /proc/meminfo から (MemTotal, MemAvailable) を kB で返す
fn parse_meminfo(content: &str) -> anyhow::Result<(u64, u64)> {
    let mut total = None;
    let mut available = None;
    for line in content.lines() {
        if let Some(rest) = line.strip_prefix("MemTotal:") {
            total = Some(parse_kb(rest)?);
        } else if let Some(rest) = line.strip_prefix("MemAvailable:") {
            available = Some(parse_kb(rest)?);
        }
    }
    Ok((
        total.context("/proc/meminfo に MemTotal がありません")?,
        available.context("/proc/meminfo に MemAvailable がありません")?,
    ))
}

fn parse_kb(rest: &str) -> anyhow::Result<u64> {
    rest.split_whitespace()
        .next()
        .context("meminfo の値が空です")?
        .parse::<u64>()
        .context("meminfo の値が数値ではありません")
}

/// available * 10 < total → High (整数演算で 10% 判定)
fn mem_level(total_kb: u64, available_kb: u64) -> MemLevel {
    if available_kb.saturating_mul(10) < total_kb {
        MemLevel::High
    } else {
        MemLevel::Normal
    }
}

/// 現在のレベルを読む。読み取り/パース失敗時は (Normal, Normal) に退避し、
/// 警告は初回 1 回のみ stderr に出す
pub fn read_levels() -> (CpuLevel, MemLevel) {
    static WARNED: AtomicBool = AtomicBool::new(false);
    match try_read_levels() {
        Ok(levels) => levels,
        Err(err) => {
            if !WARNED.swap(true, Ordering::Relaxed) {
                eprintln!("miryam: システム状態の取得に失敗しました (normal 扱いで続行): {err:#}");
            }
            (CpuLevel::Normal, MemLevel::Normal)
        }
    }
}

fn try_read_levels() -> anyhow::Result<(CpuLevel, MemLevel)> {
    let cores = std::thread::available_parallelism()
        .map(|n| n.get() as u32)
        .unwrap_or(1);
    let load1 = parse_loadavg(
        &std::fs::read_to_string("/proc/loadavg").context("/proc/loadavg を読めません")?,
    )?;
    let (total, available) = parse_meminfo(
        &std::fs::read_to_string("/proc/meminfo").context("/proc/meminfo を読めません")?,
    )?;
    Ok((cpu_level(load1, cores), mem_level(total, available)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_loadavg_first_field() {
        assert_eq!(parse_loadavg("1.33 0.76 0.72 3/4099 123456").unwrap(), 1.33);
        assert_eq!(parse_loadavg("0.00 0.00 0.00 1/100 1").unwrap(), 0.0);
    }

    #[test]
    fn rejects_bad_loadavg() {
        assert!(parse_loadavg("").is_err());
        assert!(parse_loadavg("abc 0.5 0.5").is_err());
    }

    #[test]
    fn cpu_level_boundaries() {
        assert_eq!(cpu_level(0.99, 10), CpuLevel::Idle, "r=0.099 < 0.1");
        assert_eq!(cpu_level(1.0, 10), CpuLevel::Normal, "r=0.1 は Idle でない");
        assert_eq!(cpu_level(7.99, 10), CpuLevel::Normal, "r=0.799");
        assert_eq!(cpu_level(8.0, 10), CpuLevel::High, "r=0.8 は High");
        assert_eq!(cpu_level(0.05, 1), CpuLevel::Idle);
    }

    #[test]
    fn parses_meminfo_fields() {
        let content = "MemTotal:       28471296 kB\nMemFree:         3659776 kB\nMemAvailable:    7569408 kB\nBuffers:          123456 kB\n";
        assert_eq!(parse_meminfo(content).unwrap(), (28471296, 7569408));
    }

    #[test]
    fn rejects_meminfo_missing_fields() {
        assert!(parse_meminfo("MemTotal:       28471296 kB\n").is_err());
        assert!(parse_meminfo("MemAvailable:    7569408 kB\n").is_err());
        assert!(parse_meminfo("").is_err());
    }

    #[test]
    fn mem_level_boundary() {
        assert_eq!(mem_level(1000, 99), MemLevel::High, "9.9% は High");
        assert_eq!(mem_level(1000, 100), MemLevel::Normal, "ちょうど 10% は Normal");
        assert_eq!(mem_level(1000, 500), MemLevel::Normal);
    }
}
