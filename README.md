# Pulse for Linux

A process and resource monitor built with Rust, GTK 4 and libadwaita.

| Tab | What it shows |
|---|---|
| Processes | Every process with its user, CPU, memory, disk I/O and threads. Sort by any column, search by name, PID, user or command line, and end a process. |
| Devices | Load per processor, per physical disk and per network interface. |
| Disk Space | The folders and files that take up the space under a folder you choose, with drill-down. |

Four cards above the tabs follow CPU, memory, disk and network for the last minute.

## Build and run

Requires a recent stable Rust (built and checked with 1.97), GTK 4.12+ and libadwaita 1.5+.

```sh
# Debian / Ubuntu
sudo apt install libgtk-4-dev libadwaita-1-dev
# Fedora
sudo dnf install gtk4-devel libadwaita-devel

cargo run
cargo test
```

## Layout

| Path | Contents |
|---|---|
| `src/main.rs` | Application ID, application actions, About dialog. |
| `src/window.rs` | Window layout, window actions, the sampling loop. |
| `src/telemetry/` | Sampling through procfs and sysfs, user names, the disk space scanner. No UI code. |
| `src/ui/` | Widgets: metric card, sparkline, process table, devices page, disk space page. |
| `src/format.rs` | Sizes, rates and percentages as text. |
| `data/` | Desktop entry, AppStream metadata and icon. |

## How it works

- A worker thread samples once a second and sends each `Frame` to the main loop over a channel. The UI never reads `/proc` itself.
- A process is identified by its PID and start time, so a reused PID is never mistaken for the old process.
- “End Process” opens a pidfd, checks the start time and only then sends the signal through that descriptor.
- Disk and network totals count hardware devices only. Partitions, device-mapper volumes, bridges and VPN tunnels repeat traffic that already crosses a hardware device.
- Tables keep one object per row and update its properties in place, which keeps selection and scroll position stable.
- A disk space scan starts only on request. It stays on one filesystem, never follows symbolic links, counts hard-linked files once and reports allocated size, like `du -x`.
- Kernel threads are hidden until “Show Kernel Threads” is switched on in the menu.

## Limits

- Disk I/O per process is readable only for your own processes unless Pulse runs as root.
- Ending another user's process needs privileges Pulse does not ask for yet.

## Not here yet

- Translations (gettext), a Flatpak manifest and a Meson build that installs the files in `data/`.
- A privileged helper behind polkit for actions on other users' processes and for eBPF tracing.
- GPU, services, connections and startup entries.

## Licence

GPL-3.0-or-later. See `LICENSE`.
