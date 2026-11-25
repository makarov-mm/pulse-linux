# Pulse for Linux

A process and resource monitor built with Rust, GTK 4 and libadwaita. This is the foundation: four live metric cards and a sortable, searchable process table with an “End Process” action.

## Build and run

Requires a recent stable Rust (built and checked with 1.97), GTK 4.12+ and libadwaita 1.5+.

```sh
# Debian / Ubuntu
sudo apt install libgtk-4-dev libadwaita-1-dev
# Fedora
sudo dnf install gtk4-devel libadwaita-devel

cargo run
```

## Layout

| Path | Contents |
|---|---|
| `src/main.rs` | Application ID, application actions, About dialog. |
| `src/window.rs` | Window layout, window actions, the sampling loop. |
| `src/telemetry/` | Sampling through procfs and sysfs. No UI code. |
| `src/ui/` | Widgets: metric card, sparkline, process table. |
| `src/format.rs` | Sizes, rates and percentages as text. |
| `data/` | Desktop entry, AppStream metadata and icon. |

## How it works

- A worker thread samples once a second and sends each `Frame` to the main loop over a channel. The UI never reads `/proc` itself.
- A process is identified by its PID and start time, so a reused PID is never mistaken for the old process.
- “End Process” opens a pidfd, checks the start time and only then sends SIGTERM through that descriptor.
- Disk and network totals count hardware devices only. Partitions, device-mapper volumes, bridges and VPN tunnels repeat traffic that already crosses a hardware device.
- The process table keeps one object per process and updates its properties in place, which keeps selection and scroll position stable.

## Not here yet

- Translations (gettext), a Flatpak manifest and a Meson build that installs the files in `data/`.
- A privileged helper behind polkit for actions on other users' processes and for eBPF tracing.
- Per-device views, GPU, services, connections and disk space.

## Licence

GPL-3.0-or-later. See `COPYING`.
