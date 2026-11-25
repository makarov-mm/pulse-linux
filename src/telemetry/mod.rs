// SPDX-License-Identifier: GPL-3.0-or-later

//! Samples the system through procfs and sysfs. Nothing here touches the UI.

mod procfs;

use std::collections::HashMap;
use std::io;
use std::time::{Duration, Instant};

use rustix::process::{Pid, PidfdFlags, Signal, pidfd_open, pidfd_send_signal};

/// Longer gaps (suspend, a stalled sampler) are not turned into rates.
const MAX_INTERVAL: Duration = Duration::from_secs(15);

/// A PID alone can be reused. The start time pins one process instance.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ProcessKey {
    pub pid: i32,
    /// Clock ticks after boot, as reported in `/proc/<pid>/stat`.
    pub start_time: u64,
}

#[derive(Clone, Debug)]
pub struct ProcessSample {
    pub key: ProcessKey,
    pub name: String,
    /// Share of all logical processors, 0–100. `None` until two samples exist.
    pub cpu: Option<f64>,
    /// Resident set size in bytes.
    pub memory: u64,
    pub threads: u32,
}

/// One reading of the whole system. Rates are `None` on the first sample and after a gap.
#[derive(Clone, Debug, Default)]
pub struct Frame {
    pub cpu: Option<f64>,
    pub memory_total: u64,
    pub memory_used: u64,
    pub disk_read: Option<f64>,
    pub disk_write: Option<f64>,
    pub received: Option<f64>,
    pub sent: Option<f64>,
    pub processes: Vec<ProcessSample>,
}

#[derive(Default)]
pub struct Sampler {
    last: Option<Instant>,
    cpu: Option<procfs::CpuTimes>,
    disk: Option<procfs::Transfer>,
    network: Option<procfs::Transfer>,
    processes: HashMap<ProcessKey, u64>,
}

impl Sampler {
    pub fn sample(&mut self) -> Frame {
        let now = Instant::now();
        let seconds = self
            .last
            .map(|last| now.duration_since(last))
            .filter(|elapsed| !elapsed.is_zero() && *elapsed <= MAX_INTERVAL)
            .map(|elapsed| elapsed.as_secs_f64());
        self.last = Some(now);

        let cpu = procfs::cpu_times();
        let disk = procfs::disk_transfer();
        let network = procfs::network_transfer();
        let (memory_total, memory_used) = procfs::memory().unwrap_or_default();
        let processors = procfs::processors().max(1) as f64;
        let ticks = rustix::param::clock_ticks_per_second() as f64;

        let mut current = HashMap::new();
        let mut processes = Vec::new();
        for stat in procfs::processes() {
            let cpu = seconds.zip(self.processes.get(&stat.key)).and_then(|(seconds, previous)| {
                let used = stat.cpu_ticks.checked_sub(*previous)? as f64 / ticks;
                Some((100.0 * used / seconds / processors).clamp(0.0, 100.0))
            });
            current.insert(stat.key, stat.cpu_ticks);
            processes.push(ProcessSample {
                key: stat.key,
                name: stat.name,
                cpu,
                memory: stat.resident_pages * rustix::param::page_size() as u64,
                threads: stat.threads,
            });
        }

        let frame = Frame {
            cpu: seconds.and(usage(self.cpu, cpu)),
            memory_total,
            memory_used,
            disk_read: rate(seconds, self.disk, disk, |t| t.incoming),
            disk_write: rate(seconds, self.disk, disk, |t| t.outgoing),
            received: rate(seconds, self.network, network, |t| t.incoming),
            sent: rate(seconds, self.network, network, |t| t.outgoing),
            processes,
        };

        self.cpu = cpu;
        self.disk = disk;
        self.network = network;
        self.processes = current;
        frame
    }
}

fn usage(previous: Option<procfs::CpuTimes>, current: Option<procfs::CpuTimes>) -> Option<f64> {
    let (previous, current) = previous.zip(current)?;
    let total = current.total.checked_sub(previous.total).filter(|total| *total > 0)?;
    let idle = current.idle.checked_sub(previous.idle)?.min(total);
    Some(100.0 * (total - idle) as f64 / total as f64)
}

/// A counter that went backwards (a device was removed, a counter was reset) gives no rate.
fn rate(
    seconds: Option<f64>,
    previous: Option<procfs::Transfer>,
    current: Option<procfs::Transfer>,
    field: fn(&procfs::Transfer) -> u64,
) -> Option<f64> {
    let delta = field(&current?).checked_sub(field(&previous?))?;
    Some(delta as f64 / seconds?)
}

/// Asks a process to exit with SIGTERM.
///
/// The pidfd is opened first and the start time checked afterwards, so the signal
/// cannot reach another process that took over the PID in between.
pub fn terminate(key: ProcessKey) -> io::Result<()> {
    let pid = Pid::from_raw(key.pid).ok_or(io::ErrorKind::InvalidInput)?;
    let process = pidfd_open(pid, PidfdFlags::empty())?;
    if procfs::start_time(key.pid) != Some(key.start_time) {
        return Err(io::ErrorKind::NotFound.into());
    }
    pidfd_send_signal(&process, Signal::TERM)?;
    Ok(())
}
