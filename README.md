# Pulse for Linux

A process and resource monitor built with Rust, GTK 4 and libadwaita.

| Tab | What it shows |
|---|---|
| Processes | Every process with its user, CPU, memory, disk I/O and threads. Sort by any column, search by name, PID, user or command line, and end a process. |
| Devices | Load per processor, per physical disk and per network interface. |
| Connections | Open TCP and UDP sockets with their addresses, state and the process that holds each. |
| Disk Space | The folders and files that take up the space under a folder you choose, as a map and a list, with drill-down. |
| Startup | Programs that start at login, each with a switch. |

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

`cargo run` shows the English interface. Translations, the desktop entry and the icon are installed by Meson:

```sh
sudo apt install meson ninja-build gettext desktop-file-utils appstream
meson setup builddir --prefix="$HOME/.local" --buildtype=release
meson install -C builddir
meson test -C builddir
```

## Translating

Source strings are English and go through gettext. To add a language, add its code to `po/LINGUAS`, create the catalogue and translate it:

```sh
meson compile -C builddir pulse-pot
msginit --locale=de --input=po/pulse.pot --output-file=po/de.po
```

After the sources change, `meson compile -C builddir pulse-update-po` brings every catalogue up to date. A new source file with translatable strings must be listed in `po/POTFILES.in`.

## Layout

| Path | Contents |
|---|---|
| `src/main.rs` | Application ID, application actions, About dialog. |
| `src/window.rs` | Window layout, window actions, the sampling loop. |
| `src/telemetry/` | Sampling through procfs and sysfs, user names, sockets, autostart entries, the disk space scanner. No UI code. |
| `src/ui/` | Widgets: metric card, sparkline, treemap, and the page behind each tab. |
| `src/format.rs` | Sizes, rates and percentages as text. |
| `src/i18n.rs` | gettext setup and the translation helpers. |
| `po/` | Translations. |
| `data/` | Desktop entry, AppStream metadata and icon. |
| `meson.build` | Build and installation: runs Cargo, compiles translations, installs the data files. |

## How it works

- A worker thread samples once a second and sends each `Frame` to the main loop over a channel. The UI never reads `/proc` itself.
- A process is identified by its PID and start time, so a reused PID is never mistaken for the old process.
- “End Process” opens a pidfd, checks the start time and only then sends the signal through that descriptor.
- Disk and network totals count hardware devices only. Partitions, device-mapper volumes, bridges and VPN tunnels repeat traffic that already crosses a hardware device.
- Tables keep one object per row and update its properties in place, which keeps selection and scroll position stable.
- A disk space scan starts only on request. It stays on one filesystem, never follows symbolic links, counts hard-linked files once and reports allocated size, like `du -x`.
- Sockets are read only while the Connections tab is in view, every two seconds, off the main loop.
- Startup lists the XDG autostart entries. Switching one writes `Hidden=` to a copy in `~/.config/autostart`; system files are never changed.
- Kernel threads are hidden until “Show Kernel Threads” is switched on in the menu.

## Limits

- Disk I/O per process and the owner of a socket are readable only for your own processes unless Pulse runs as root.
- Connections cover the network namespace Pulse runs in, so sockets inside containers are not listed.
- Ending another user's process needs privileges Pulse does not ask for yet.

## Not here yet

- A Flatpak manifest.
- Numbers formatted for the locale: the decimal separator is always a point.
- A privileged helper behind polkit for actions on other users' processes and for eBPF tracing.
- GPU and services. Startup does not cover systemd user units.
- A layout for narrow windows: the header needs about 1000 pixels for its five tabs.

## Licence

GPL-3.0-or-later. See `LICENSE`.
