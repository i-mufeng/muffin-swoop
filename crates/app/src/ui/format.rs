//! 人类可读的格式化辅助

pub fn bytes(n: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];
    if n < 1024 {
        return format!("{n} B");
    }
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if v >= 100.0 {
        format!("{v:.0} {}", UNITS[i])
    } else if v >= 10.0 {
        format!("{v:.1} {}", UNITS[i])
    } else {
        format!("{v:.2} {}", UNITS[i])
    }
}

pub fn speed(bps: f64) -> String {
    if bps < 1.0 {
        return "0 B/s".to_string();
    }
    format!("{}/s", bytes(bps as u64))
}

pub fn duration(secs: f64) -> String {
    if !secs.is_finite() || secs < 0.0 {
        return "--".to_string();
    }
    let s = secs.round() as u64;
    if s < 60 {
        format!("{s} 秒")
    } else if s < 3600 {
        format!("{}:{:02}", s / 60, s % 60)
    } else if s < 86_400 {
        format!("{}:{:02}:{:02}", s / 3600, (s % 3600) / 60, s % 60)
    } else {
        format!("{} 天 {} 小时", s / 86_400, (s % 86_400) / 3600)
    }
}

/// MB/s → 字节/秒
pub fn mbps_to_bps(mbps: f64) -> u64 {
    (mbps.max(0.0) * 1024.0 * 1024.0) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_format() {
        assert_eq!(bytes(0), "0 B");
        assert_eq!(bytes(1023), "1023 B");
        assert_eq!(bytes(1536), "1.50 KB");
        assert_eq!(bytes(10 * 1024 * 1024), "10.0 MB");
        assert_eq!(bytes(250 * 1024 * 1024), "250 MB");
    }

    #[test]
    fn duration_format() {
        assert_eq!(duration(5.0), "5 秒");
        assert_eq!(duration(65.0), "1:05");
        assert_eq!(duration(3665.0), "1:01:05");
    }
}
