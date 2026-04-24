use std::sync::atomic::{AtomicU8, Ordering};

use crate::config::CONFIG;

#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RetentionPolicy {
    Eager = 0,
    Moderate = 1,
    Aggressive = 2,
}

static POLICY: AtomicU8 = AtomicU8::new(RetentionPolicy::Moderate as u8);

pub fn current_policy() -> RetentionPolicy {
    match POLICY.load(Ordering::Relaxed) {
        0 => RetentionPolicy::Eager,
        2 => RetentionPolicy::Aggressive,
        _ => RetentionPolicy::Moderate,
    }
}

pub fn update_policy() {
    let policy = detect_pressure();
    POLICY.store(policy as u8, Ordering::Relaxed);
}

fn detect_pressure() -> RetentionPolicy {
    let available_kb = read_memavailable_kb();
    if available_kb == 0 {
        return RetentionPolicy::Moderate;
    }

    let rss_kb = read_rss_kb();
    if rss_kb == 0 {
        return RetentionPolicy::Moderate;
    }

    let utilization = (rss_kb * 100) / available_kb;

    if utilization <= CONFIG.pressure_eager_pct as u64 {
        RetentionPolicy::Eager
    } else if utilization >= CONFIG.pressure_aggressive_pct as u64 {
        RetentionPolicy::Aggressive
    } else {
        RetentionPolicy::Moderate
    }
}

fn read_memavailable_kb() -> u64 {
    let contents = match std::fs::read_to_string("/proc/meminfo") {
        Ok(c) => c,
        Err(_) => return 0,
    };

    for line in contents.lines() {
        if line.starts_with("MemAvailable:") {
            return parse_kb_line(line);
        }
    }
    0
}

fn read_rss_kb() -> u64 {
    let contents = match std::fs::read_to_string("/proc/self/status") {
        Ok(c) => c,
        Err(_) => return 0,
    };

    for line in contents.lines() {
        if line.starts_with("VmRSS:") {
            return parse_kb_line(line);
        }
    }
    0
}

fn parse_kb_line(line: &str) -> u64 {
    line.split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}
