// SPDX-License-Identifier: GPL-3.0-or-later

//! Parsers for the procfs and sysfs files the sampler reads.
//! Formats are described in proc(5) and the kernel's Documentation/admin-guide/iostats.rst.

use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

use super::ProcessKey;

/// `/proc/diskstats` always counts in 512-byte sectors, whatever the device uses.
const SECTOR_BYTES: u64 = 512;
/// `PF_KTHREAD` in the flags field of `/proc/<pid>/stat`.
const KERNEL_THREAD: u64 = 0x0020_0000;

#[derive(Clone, Copy)]
pub struct CpuTimes {
    pub total: u64,
    /// Idle and I/O wait together.
    pub idle: u64,
}

pub struct CpuSnapshot {
    pub all: CpuTimes,
    /// Online processors by their kernel number.
    pub cores: Vec<(usize, CpuTimes)>,
}

/// Cumulative bytes in each direction: read and written, or received and sent.
#[derive(Clone, Copy, Default)]
pub struct Transfer {
    pub incoming: u64,
    pub outgoing: u64,
}

pub struct Device {
    pub name: String,
    pub transfer: Transfer,
    /// Backed by a hardware device rather than stacked on another one.
    pub hardware: bool,
}

pub struct ProcessStat {
    pub key: ProcessKey,
    pub name: String,
    pub uid: u32,
    pub kernel: bool,
    pub cpu_ticks: u64,
    pub resident_pages: u64,
    pub threads: u32,
    /// Bytes this process caused to be read from and written to storage.
    /// Only readable for the user's own processes, or as root.
    pub io: Option<Transfer>,
}

pub fn cpu() -> Option<CpuSnapshot> {
    let stat = fs::read_to_string("/proc/stat").ok()?;
    let mut all = None;
    let mut cores = Vec::new();
    for line in stat.lines() {
        let Some(rest) = line.strip_prefix("cpu") else { continue };
        let Some((label, values)) = rest.split_once(' ') else {
            continue;
        };
        // user nice system idle iowait irq softirq steal. Guest time is already part of user and nice.
        let fields: Vec<u64> = values
            .split_ascii_whitespace()
            .take(8)
            .filter_map(|field| field.parse().ok())
            .collect();
        if fields.len() < 4 {
            continue;
        }
        let times = CpuTimes {
            total: fields.iter().sum(),
            idle: fields[3] + fields.get(4).copied().unwrap_or(0),
        };
        match label.parse::<usize>() {
            Ok(number) => cores.push((number, times)),
            Err(_) if label.is_empty() => all = Some(times),
            Err(_) => {}
        }
    }
    Some(CpuSnapshot { all: all?, cores })
}

/// Total and used memory in bytes. Used is what `free` reports: total minus available.
pub fn memory() -> Option<(u64, u64)> {
    let info = fs::read_to_string("/proc/meminfo").ok()?;
    let field = |name: &str| -> Option<u64> {
        let line = info
            .lines()
            .find_map(|line| line.strip_prefix(name)?.strip_prefix(':'))?;
        Some(line.split_ascii_whitespace().next()?.parse::<u64>().ok()? * 1024)
    };
    let total = field("MemTotal")?;
    Some((total, total.saturating_sub(field("MemAvailable")?)))
}

/// Physical disks with the bytes read from and written to them.
///
/// Partitions and stacked devices (device mapper, md, loop) repeat the I/O of the disk
/// below them, so only block devices backed by hardware are listed.
pub fn disks() -> Option<Vec<Device>> {
    let stats = fs::read_to_string("/proc/diskstats").ok()?;
    let mut disks = Vec::new();
    for line in stats.lines() {
        let fields: Vec<&str> = line.split_ascii_whitespace().collect();
        // major minor name reads merged sectors-read ms writes merged sectors-written ...
        if fields.len() < 10 || !Path::new("/sys/block").join(fields[2]).join("device").exists() {
            continue;
        }
        disks.push(Device {
            name: fields[2].to_owned(),
            transfer: Transfer {
                incoming: fields[5].parse::<u64>().unwrap_or(0) * SECTOR_BYTES,
                outgoing: fields[9].parse::<u64>().unwrap_or(0) * SECTOR_BYTES,
            },
            hardware: true,
        });
    }
    Some(disks)
}

/// Network interfaces other than loopback, with the bytes received and sent.
pub fn interfaces() -> Option<Vec<Device>> {
    let table = fs::read_to_string("/proc/net/dev").ok()?;
    let mut interfaces = Vec::new();
    for line in table.lines().skip(2) {
        let Some((name, counters)) = line.split_once(':') else {
            continue;
        };
        let name = name.trim();
        let fields: Vec<&str> = counters.split_ascii_whitespace().collect();
        // Receive: bytes packets errs drop fifo frame compressed multicast. Transmit: bytes ...
        if name == "lo" || fields.len() < 9 {
            continue;
        }
        interfaces.push(Device {
            name: name.to_owned(),
            transfer: Transfer {
                incoming: fields[0].parse().unwrap_or(0),
                outgoing: fields[8].parse().unwrap_or(0),
            },
            hardware: Path::new("/sys/class/net").join(name).join("device").exists(),
        });
    }
    Some(interfaces)
}

/// Every process that could be read. One that exits during the walk is left out.
pub fn processes() -> Vec<ProcessStat> {
    let Ok(entries) = fs::read_dir("/proc") else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| entry.file_name().to_str()?.parse::<i32>().ok())
        .filter_map(process)
        .collect()
}

pub fn start_time(pid: i32) -> Option<u64> {
    process(pid).map(|stat| stat.key.start_time)
}

/// What the details dialog shows beyond the sampled values.
pub struct ProcessInfo {
    /// The one-letter state from `/proc/<pid>/stat`, such as `R` or `S`.
    pub state: char,
    pub parent: i32,
    pub nice: i64,
    pub executable: Option<String>,
    pub directory: Option<String>,
    pub open_files: Option<usize>,
}

/// Reads the details of one process instance. `None` when it has exited or its PID was reused.
pub fn info(key: ProcessKey) -> Option<ProcessInfo> {
    let directory = format!("/proc/{}", key.pid);
    let stat = fs::read_to_string(format!("{directory}/stat")).ok()?;
    let fields: Vec<&str> = stat.rsplit_once(')')?.1.split_ascii_whitespace().collect();
    // As in `process`, field N of proc(5) is at index N - 3.
    let field = |number: usize| fields.get(number - 3).copied();
    if field(22)?.parse::<u64>().ok()? != key.start_time {
        return None;
    }
    let link = |name: &str| {
        fs::read_link(format!("{directory}/{name}"))
            .ok()
            .map(|path| path.to_string_lossy().into_owned())
    };
    Some(ProcessInfo {
        state: field(3)?.chars().next()?,
        parent: field(4)?.parse().ok()?,
        nice: field(19)?.parse().ok()?,
        // These three are readable for the user's own processes, or as root.
        executable: link("exe"),
        directory: link("cwd"),
        open_files: fs::read_dir(format!("{directory}/fd")).ok().map(Iterator::count),
    })
}

/// Seconds since the Unix epoch at which the system booted.
pub fn boot_time() -> Option<u64> {
    let stat = fs::read_to_string("/proc/stat").ok()?;
    stat.lines()
        .find_map(|line| line.strip_prefix("btime ")?.trim().parse().ok())
}

/// The arguments the process was started with, separated by spaces. Empty for kernel threads.
pub fn command_line(pid: i32) -> String {
    let Ok(mut raw) = fs::read(format!("/proc/{pid}/cmdline")) else {
        return String::new();
    };
    while raw.last() == Some(&0) {
        raw.pop();
    }
    raw.iter_mut().filter(|byte| **byte == 0).for_each(|byte| *byte = b' ');
    String::from_utf8_lossy(&raw).into_owned()
}

fn process(pid: i32) -> Option<ProcessStat> {
    let directory = format!("/proc/{pid}");
    let stat = fs::read_to_string(format!("{directory}/stat")).ok()?;
    // The name sits in parentheses and may itself contain spaces and parentheses,
    // so the numbered fields are found from the last closing one.
    let (head, tail) = stat.rsplit_once(')')?;
    let name = head.split_once('(')?.1.to_owned();
    // `tail` starts at field 3 (state), so field N of proc(5) is at index N - 3.
    let fields: Vec<&str> = tail.split_ascii_whitespace().collect();
    let number = |field: usize| fields.get(field - 3)?.parse::<u64>().ok();
    Some(ProcessStat {
        key: ProcessKey {
            pid,
            start_time: number(22)?,
        },
        name,
        // The directory belongs to the effective user of the process.
        uid: fs::metadata(&directory).ok()?.uid(),
        kernel: number(9)? & KERNEL_THREAD != 0,
        cpu_ticks: number(14)? + number(15)?,
        threads: number(20)? as u32,
        resident_pages: number(24)?,
        io: process_io(&directory),
    })
}

fn process_io(directory: &str) -> Option<Transfer> {
    let io = fs::read_to_string(format!("{directory}/io")).ok()?;
    let field = |name: &str| {
        io.lines()
            .find_map(|line| line.strip_prefix(name)?.trim().parse::<u64>().ok())
    };
    Some(Transfer {
        incoming: field("read_bytes:")?,
        outgoing: field("write_bytes:")?,
    })
}
