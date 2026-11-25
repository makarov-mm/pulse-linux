// SPDX-License-Identifier: GPL-3.0-or-later

//! Parsers for the procfs and sysfs files the sampler reads.
//! Formats are described in proc(5) and the kernel's Documentation/admin-guide/iostats.rst.

use std::fs;
use std::path::Path;

use super::ProcessKey;

/// `/proc/diskstats` always counts in 512-byte sectors, whatever the device uses.
const SECTOR_BYTES: u64 = 512;

#[derive(Clone, Copy)]
pub struct CpuTimes {
    pub total: u64,
    /// Idle and I/O wait together.
    pub idle: u64,
}

/// Cumulative bytes in each direction: read and written, or received and sent.
#[derive(Clone, Copy, Default)]
pub struct Transfer {
    pub incoming: u64,
    pub outgoing: u64,
}

pub struct ProcessStat {
    pub key: ProcessKey,
    pub name: String,
    pub cpu_ticks: u64,
    pub resident_pages: u64,
    pub threads: u32,
}

pub fn cpu_times() -> Option<CpuTimes> {
    let stat = fs::read_to_string("/proc/stat").ok()?;
    // user nice system idle iowait irq softirq steal. Guest time is already part of user and nice.
    let fields: Vec<u64> = stat
        .lines()
        .next()?
        .split_ascii_whitespace()
        .skip(1)
        .take(8)
        .filter_map(|field| field.parse().ok())
        .collect();
    if fields.len() < 4 {
        return None;
    }
    let idle = fields[3] + fields.get(4).copied().unwrap_or(0);
    Some(CpuTimes { total: fields.iter().sum(), idle })
}

/// The number of processors that are online.
pub fn processors() -> usize {
    fs::read_to_string("/proc/stat").map_or(0, |stat| {
        stat.lines()
            .filter(|line| line.strip_prefix("cpu").is_some_and(|rest| rest.starts_with(|c: char| c.is_ascii_digit())))
            .count()
    })
}

/// Total and used memory in bytes. Used is what `free` reports: total minus available.
pub fn memory() -> Option<(u64, u64)> {
    let info = fs::read_to_string("/proc/meminfo").ok()?;
    let field = |name: &str| -> Option<u64> {
        let line = info.lines().find_map(|line| line.strip_prefix(name)?.strip_prefix(':'))?;
        Some(line.split_ascii_whitespace().next()?.parse::<u64>().ok()? * 1024)
    };
    let total = field("MemTotal")?;
    Some((total, total.saturating_sub(field("MemAvailable")?)))
}

/// Bytes read from and written to physical disks.
///
/// Partitions and stacked devices (device mapper, md, loop) repeat the I/O of the disk
/// below them, so only block devices backed by hardware are counted.
pub fn disk_transfer() -> Option<Transfer> {
    let stats = fs::read_to_string("/proc/diskstats").ok()?;
    let mut total = Transfer::default();
    for line in stats.lines() {
        let fields: Vec<&str> = line.split_ascii_whitespace().collect();
        // major minor name reads merged sectors-read ms writes merged sectors-written ...
        if fields.len() < 10 || !Path::new("/sys/block").join(fields[2]).join("device").exists() {
            continue;
        }
        total.incoming += fields[5].parse::<u64>().unwrap_or(0) * SECTOR_BYTES;
        total.outgoing += fields[9].parse::<u64>().unwrap_or(0) * SECTOR_BYTES;
    }
    Some(total)
}

/// Bytes received and sent over network interfaces.
///
/// Traffic of bridges, VPN tunnels and container interfaces also crosses a hardware
/// interface, so only those are counted. A machine without any uses every interface but loopback.
pub fn network_transfer() -> Option<Transfer> {
    let table = fs::read_to_string("/proc/net/dev").ok()?;
    let (mut hardware, mut other) = (Transfer::default(), Transfer::default());
    let mut found_hardware = false;
    for line in table.lines().skip(2) {
        let Some((name, counters)) = line.split_once(':') else { continue };
        let name = name.trim();
        let fields: Vec<&str> = counters.split_ascii_whitespace().collect();
        // Receive: bytes packets errs drop fifo frame compressed multicast. Transmit: bytes ...
        if name == "lo" || fields.len() < 9 {
            continue;
        }
        let is_hardware = Path::new("/sys/class/net").join(name).join("device").exists();
        found_hardware |= is_hardware;
        let target = if is_hardware { &mut hardware } else { &mut other };
        target.incoming += fields[0].parse::<u64>().unwrap_or(0);
        target.outgoing += fields[8].parse::<u64>().unwrap_or(0);
    }
    Some(if found_hardware { hardware } else { other })
}

/// Every process that could be read. One that exits during the walk is left out.
pub fn processes() -> Vec<ProcessStat> {
    let Ok(entries) = fs::read_dir("/proc") else { return Vec::new() };
    entries
        .flatten()
        .filter_map(|entry| entry.file_name().to_str()?.parse::<i32>().ok())
        .filter_map(process)
        .collect()
}

pub fn start_time(pid: i32) -> Option<u64> {
    process(pid).map(|stat| stat.key.start_time)
}

fn process(pid: i32) -> Option<ProcessStat> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // The name sits in parentheses and may itself contain spaces and parentheses,
    // so the numbered fields are found from the last closing one.
    let (head, tail) = stat.rsplit_once(')')?;
    let name = head.split_once('(')?.1.to_owned();
    // `tail` starts at field 3 (state), so field N of proc(5) is at index N - 3.
    let fields: Vec<&str> = tail.split_ascii_whitespace().collect();
    let number = |field: usize| fields.get(field - 3)?.parse::<u64>().ok();
    Some(ProcessStat {
        key: ProcessKey { pid, start_time: number(22)? },
        name,
        cpu_ticks: number(14)? + number(15)?,
        threads: number(20)? as u32,
        resident_pages: number(24)?,
    })
}
