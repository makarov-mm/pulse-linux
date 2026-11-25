// SPDX-License-Identifier: GPL-3.0-or-later

//! Samples the system through procfs and sysfs. Nothing here touches the UI.

pub mod autostart;
mod procfs;
pub mod sockets;
pub mod space;
mod users;

use std::collections::HashMap;
use std::io;
use std::sync::Arc;
use std::time::{Duration, Instant};

use rustix::process::{Pid, PidfdFlags, Signal, pidfd_open, pidfd_send_signal};

use procfs::{CpuTimes, Device, Transfer};

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
    /// PID of the parent, 0 for the processes the kernel started.
    pub parent: i32,
    pub name: String,
    pub user: Arc<str>,
    /// The command line, or the bracketed name for a kernel thread.
    pub command: Arc<str>,
    pub kernel: bool,
    /// Share of all logical processors, 0–100. `None` until two samples exist.
    pub cpu: Option<f64>,
    /// Resident set size in bytes.
    pub memory: u64,
    /// Bytes per second read from and written to storage. `None` when the process cannot be read.
    pub disk: Option<f64>,
    pub threads: u32,
}

#[derive(Clone, Debug)]
pub struct CoreSample {
    pub number: usize,
    pub usage: Option<f64>,
}

/// Bytes per second through one disk or network interface.
#[derive(Clone, Debug)]
pub struct DeviceSample {
    pub name: String,
    pub incoming: Option<f64>,
    pub outgoing: Option<f64>,
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
    pub cores: Vec<CoreSample>,
    pub disks: Vec<DeviceSample>,
    pub interfaces: Vec<DeviceSample>,
    pub processes: Vec<ProcessSample>,
}

/// What is remembered about a process between samples.
struct Known {
    cpu_ticks: u64,
    io: Option<Transfer>,
    command: Arc<str>,
}

#[derive(Default)]
pub struct Sampler {
    last: Option<Instant>,
    cpu: Option<CpuTimes>,
    cores: HashMap<usize, CpuTimes>,
    disks: HashMap<String, Transfer>,
    interfaces: HashMap<String, Transfer>,
    processes: HashMap<ProcessKey, Known>,
    users: users::Users,
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

        let mut frame = Frame::default();
        self.sample_cpu(seconds, &mut frame);
        (frame.memory_total, frame.memory_used) = procfs::memory().unwrap_or_default();

        let disks = procfs::disks().unwrap_or_default();
        (frame.disks, frame.disk_read, frame.disk_write) = rates(seconds, &mut self.disks, &disks);
        let interfaces = procfs::interfaces().unwrap_or_default();
        (frame.interfaces, frame.received, frame.sent) = rates(seconds, &mut self.interfaces, &interfaces);

        self.sample_processes(seconds, &mut frame);
        frame
    }

    fn sample_cpu(&mut self, seconds: Option<f64>, frame: &mut Frame) {
        let Some(snapshot) = procfs::cpu() else {
            self.cpu = None;
            self.cores.clear();
            return;
        };
        frame.cpu = seconds.and(usage(self.cpu, snapshot.all));
        frame.cores = snapshot
            .cores
            .iter()
            .map(|(number, times)| CoreSample {
                number: *number,
                usage: seconds.and(usage(self.cores.get(number).copied(), *times)),
            })
            .collect();
        self.cpu = Some(snapshot.all);
        self.cores = snapshot.cores.into_iter().collect();
    }

    fn sample_processes(&mut self, seconds: Option<f64>, frame: &mut Frame) {
        let processors = frame.cores.len().max(1) as f64;
        let ticks = rustix::param::clock_ticks_per_second() as f64;
        let page = rustix::param::page_size() as u64;
        let mut current = HashMap::new();

        for stat in procfs::processes() {
            let known = self.processes.remove(&stat.key);
            let cpu = seconds.zip(known.as_ref()).and_then(|(seconds, known)| {
                let used = stat.cpu_ticks.checked_sub(known.cpu_ticks)? as f64 / ticks;
                Some((100.0 * used / seconds / processors).clamp(0.0, 100.0))
            });
            let disk = seconds.zip(known.as_ref()).and_then(|(seconds, known)| {
                let (now, before) = (stat.io?, known.io?);
                let read = now.incoming.checked_sub(before.incoming)?;
                let written = now.outgoing.checked_sub(before.outgoing)?;
                Some((read + written) as f64 / seconds)
            });
            // The command line is read once: it rarely changes and costs a file read per process.
            let command = known.map(|known| known.command).unwrap_or_else(|| {
                let line = procfs::command_line(stat.key.pid);
                if line.is_empty() {
                    format!("[{}]", stat.name).into()
                } else {
                    line.into()
                }
            });

            current.insert(
                stat.key,
                Known {
                    cpu_ticks: stat.cpu_ticks,
                    io: stat.io,
                    command: command.clone(),
                },
            );
            frame.processes.push(ProcessSample {
                key: stat.key,
                parent: stat.parent,
                name: stat.name,
                user: self.users.name(stat.uid),
                command,
                kernel: stat.kernel,
                cpu,
                memory: stat.resident_pages * page,
                disk,
                threads: stat.threads,
            });
        }
        self.processes = current;
    }
}

fn usage(previous: Option<CpuTimes>, current: CpuTimes) -> Option<f64> {
    let previous = previous?;
    let total = current.total.checked_sub(previous.total).filter(|total| *total > 0)?;
    let idle = current.idle.checked_sub(previous.idle)?.min(total);
    Some(100.0 * (total - idle) as f64 / total as f64)
}

/// Per-device rates and their totals, replacing `previous` with the new counters.
///
/// Traffic of stacked devices (bridges, VPN tunnels) also crosses a hardware device, so the
/// totals count hardware only. A machine without any counts every device.
/// A counter that went backwards (a device was replaced, a counter reset) gives no rate.
fn rates(
    seconds: Option<f64>,
    previous: &mut HashMap<String, Transfer>,
    devices: &[Device],
) -> (Vec<DeviceSample>, Option<f64>, Option<f64>) {
    let hardware_only = devices.iter().any(|device| device.hardware);
    let (mut incoming, mut outgoing) = (None, None);
    let mut samples = Vec::with_capacity(devices.len());

    for device in devices {
        let before = previous.get(&device.name);
        let rate = |now: u64, before: Option<u64>| Some(now.checked_sub(before?)? as f64 / seconds?);
        let sample = DeviceSample {
            name: device.name.clone(),
            incoming: rate(device.transfer.incoming, before.map(|t| t.incoming)),
            outgoing: rate(device.transfer.outgoing, before.map(|t| t.outgoing)),
        };
        if device.hardware || !hardware_only {
            if let Some(rate) = sample.incoming {
                incoming = Some(incoming.unwrap_or(0.0) + rate);
            }
            if let Some(rate) = sample.outgoing {
                outgoing = Some(outgoing.unwrap_or(0.0) + rate);
            }
        }
        samples.push(sample);
    }

    *previous = devices
        .iter()
        .map(|device| (device.name.clone(), device.transfer))
        .collect();
    (samples, incoming, outgoing)
}

/// The details of one process, read when its dialog opens.
#[derive(Clone, Debug)]
pub struct ProcessDetails {
    /// The one-letter state from `/proc/<pid>/stat`, such as `R` or `S`.
    pub state: char,
    pub parent: i32,
    pub parent_name: String,
    pub nice: i64,
    /// Seconds since the Unix epoch.
    pub started: Option<i64>,
    pub executable: Option<String>,
    pub directory: Option<String>,
    pub open_files: Option<usize>,
    pub command: String,
}

/// `None` when the process has exited.
pub fn details(key: ProcessKey) -> Option<ProcessDetails> {
    let info = procfs::info(key)?;
    let ticks = rustix::param::clock_ticks_per_second();
    Some(ProcessDetails {
        state: info.state,
        parent: info.parent,
        parent_name: std::fs::read_to_string(format!("/proc/{}/comm", info.parent))
            .map(|name| name.trim_end().to_owned())
            .unwrap_or_default(),
        nice: info.nice,
        started: procfs::boot_time().and_then(|boot| i64::try_from(boot + key.start_time / ticks).ok()),
        executable: info.executable,
        directory: info.directory,
        open_files: info.open_files,
        command: procfs::command_line(key.pid),
    })
}

/// Asks a process to exit with SIGTERM, or stops it at once with SIGKILL when `force` is set.
///
/// The pidfd is opened first and the start time checked afterwards, so the signal
/// cannot reach another process that took over the PID in between.
pub fn terminate(key: ProcessKey, force: bool) -> io::Result<()> {
    let pid = Pid::from_raw(key.pid).ok_or(io::ErrorKind::InvalidInput)?;
    let process = pidfd_open(pid, PidfdFlags::empty())?;
    if procfs::start_time(key.pid) != Some(key.start_time) {
        return Err(io::ErrorKind::NotFound.into());
    }
    pidfd_send_signal(&process, if force { Signal::KILL } else { Signal::TERM })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::process::{Command, Stdio};

    use super::*;

    fn spawn_sleeper() -> std::process::Child {
        Command::new("sleep")
            .arg("600")
            .stdout(Stdio::null())
            .spawn()
            .expect("sleep starts")
    }

    #[test]
    fn terminate_ends_the_matching_process() {
        let mut child = spawn_sleeper();
        let pid = child.id() as i32;
        let key = ProcessKey {
            pid,
            start_time: procfs::start_time(pid).expect("the child is readable"),
        };
        terminate(key, false).expect("the signal is sent");
        assert!(!child.wait().expect("the child is reaped").success());
    }

    #[test]
    fn terminate_spares_a_process_with_another_start_time() {
        let mut child = spawn_sleeper();
        let pid = child.id() as i32;
        let start_time = procfs::start_time(pid).expect("the child is readable");
        let stale = ProcessKey {
            pid,
            start_time: start_time + 1,
        };
        assert_eq!(
            terminate(stale, true).map_err(|error| error.kind()),
            Err(io::ErrorKind::NotFound)
        );
        assert!(child.try_wait().expect("the child can be polled").is_none());
        child.kill().expect("the child is cleaned up");
        child.wait().expect("the child is reaped");
    }

    #[test]
    fn details_describe_this_process_and_reject_a_stale_key() {
        let pid = std::process::id() as i32;
        let key = ProcessKey {
            pid,
            start_time: procfs::start_time(pid).expect("this process is readable"),
        };
        let own = details(key).expect("this process has details");
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("the clock is set");
        assert!(own.executable.is_some() && own.open_files.is_some_and(|count| count > 0) && !own.command.is_empty());
        assert!(
            own.started
                .is_some_and(|started| started <= now.as_secs() as i64 && started > 0)
        );
        assert!(
            details(ProcessKey {
                pid,
                start_time: key.start_time + 1
            })
            .is_none()
        );
    }

    #[test]
    fn second_sample_measures_this_process() {
        let mut sampler = Sampler::default();
        let first = sampler.sample();
        assert!(first.cpu.is_none() && first.memory_total > 0);
        std::thread::sleep(Duration::from_millis(50));
        let second = sampler.sample();
        let own = second
            .processes
            .iter()
            .find(|process| process.key.pid == std::process::id() as i32);
        assert!(second.cpu.is_some() && !second.cores.is_empty());
        assert!(own.is_some_and(|process| process.cpu.is_some() && process.memory > 0 && !process.kernel));
    }
}
