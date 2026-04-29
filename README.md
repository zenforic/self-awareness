# self-awareness

A lightweight, stealth screen-logging daemon for Windows. Runs invisibly in the background, captures screenshots at a configurable interval, and ships with a terminal UI (TUI) for managing all settings — no tray icon, no taskbar presence, no fuss.

> Like Windows Recall, but the AI integration is optional and *you're* in control of your data.

---

## Features

- **Invisible operation** — console window is hidden; no tray icon or taskbar entry
- **Zero data loss** — screenshots are written to disk immediately; nothing is buffered in memory
- **Configurable capture interval** — from seconds to hours
- **Multiple image formats** — WebP (default, ~95% quality), JPEG, or PNG
- **Automatic disk management** — deletes the oldest images when the configured size cap is reached
- **Retention policy** — a scheduled cleanup task prunes images older than N days, independent of the daemon
- **Startup persistence** — optional Windows Task Scheduler task (`SelfAwarenessStartup`) launches the daemon at logon
- **Full TUI** — configure everything, start/stop the daemon, and manage tasks from a terminal interface
- **Single-instance safety** — named mutex + PID file prevent duplicate daemon processes
- **Pure stable Rust** — GDI BitBlt screen capture, no nightly compiler required

---

## Use Cases

- **Away monitoring** — know what happened on your PC while you were gone
- **Timelapse creation** — build a visual record of a work session or a long-running process
- **Chain of evidence** — maintain a timestamped, tamper-evident record of on-screen activity
- **Parental / device oversight** — keep a passive log on a shared or managed machine
- **Workflow auditing** — review exactly what was done and when, after the fact

---

## Requirements

- Windows 10 or later
- Rust stable toolchain (for building from source)

---

## Building

```bash
# Quick compilation check
cargo check

# Debug build (for testing)
cargo build

# Release build (recommended for production)
cargo build --release
```

The release profile enables LTO, dead-code stripping, and maximum optimisation for a small, fast binary.

---

## Usage

```
self-awareness.exe           # Auto-detect mode (see below)
self-awareness.exe --tui     # Force TUI (short: -t)
self-awareness.exe --daemon  # Force daemon (short: -d)
```

### Auto-detect mode

| Situation | Behaviour |
|---|---|
| No daemon running | Acquires mutex → starts daemon (hidden) |
| Daemon already running | Opens TUI attached to the running daemon |
| Stale PID (process died) | Opens TUI showing **Stopped (Died)** |
| Another instance holds mutex | Opens TUI |

---

## TUI

Launch the TUI with `self-awareness.exe --tui` (or let auto-detect open it).

### Main page

| Key | Action |
|---|---|
| `Tab` / `Shift+Tab` | Navigate fields |
| `Enter` | Edit selected field / confirm edit |
| `Esc` | Cancel edit · quit TUI (daemon keeps running) |
| `S` | Save configuration |
| `D` | Start daemon (stays in TUI) |
| `X` | Stop daemon (stays in TUI) |
| `C` | Toggle startup-on-boot task |
| `T` | Switch to Tasks page |
| `E` | Exit TUI (daemon keeps running) |
| `Q` | Stop daemon and exit |

After saving with `S`, the TUI asks **"Restart daemon? [Y/N]"** — pressing `Y` stops the old daemon, regenerates the cleanup script, waits 500 ms, and starts a fresh daemon.

### Tasks page (`T`)

| Key | Action |
|---|---|
| `S` | Toggle `SelfAwarenessStartup` task |
| `C` | Toggle `SelfAwarenessCleanup` task |
| `A` | Disable both tasks |
| `T` / `Esc` | Back to main page |
| `E` | Exit TUI (daemon keeps running) |
| `Q` | Stop daemon and exit |

---

## Configuration

Settings are stored in `%APPDATA%\self-awareness\config.json`:

```json
{
  "interval_seconds": 60,
  "max_disk_mb": 500,
  "output_dir": "C:\\Users\\<you>\\Pictures\\self-awareness",
  "image_format": "webp",
  "retention_days": 7,
  "start_on_boot": false
}
```

| Field | Description | Default |
|---|---|---|
| `interval_seconds` | Seconds between screenshots | `60` |
| `max_disk_mb` | Maximum storage used (oldest files deleted first) | `500` |
| `output_dir` | Directory where screenshots are saved | `%USERPROFILE%\Pictures\self-awareness` |
| `image_format` | Output format: `webp`, `jpeg`, or `png` | `webp` |
| `retention_days` | Delete images older than N days (via cleanup task) | `7` |
| `start_on_boot` | Whether the startup task is currently enabled | `false` |

Relative paths entered in the TUI are automatically converted to absolute paths on save.

---

## File Layout

| Path | Purpose |
|---|---|
| `%APPDATA%\self-awareness\config.json` | Configuration |
| `%APPDATA%\self-awareness\daemon.pid` | PID written by daemon, read by TUI to stop it |
| `%APPDATA%\self-awareness\daemon.log` | Error log written by the daemon |
| `%APPDATA%\self-awareness\cleanup.bat` | Batch script run daily by the cleanup task |
| Windows Task Scheduler | `SelfAwarenessStartup` — logon task; `SelfAwarenessCleanup` — daily cleanup at 02:00 |
| `output_dir` (configurable) | Captured screenshots |

---

## How It Works

### Screen capture

Screenshots are taken with the Windows GDI API (`BitBlt` + `GetDIBits`). Pixel data arrives in BGRA order and is converted to RGBA before being handed to the `image` crate for encoding. Using a negative `biHeight` in the `BITMAPINFOHEADER` ensures top-down row order, matching the captured layout exactly.

### Disk management

After every capture the daemon scans the output directory, sorts image files by modification time (oldest first), and removes files until the total size falls below `max_disk_mb`. The cleanup task provides an independent, time-based retention sweep using `forfiles`.

### Startup persistence

Both scheduled tasks are created via `schtasks /Create`. The startup task runs `self-awareness.exe --daemon`, bypassing all PID / mutex auto-detect logic for a reliable hidden start. Tasks run with standard user privileges — no elevation required.

> **One-time elevation:** Creating or removing scheduled tasks via the TUI's Tasks page requires administrator privileges the first time. Run `self-awareness.exe --tui` from an elevated prompt (right-click → *Run as administrator*) once to register the tasks, then close it. From that point on the daemon starts at logon automatically and the TUI can be used normally without elevation.

---

## Security Notes

> ⚠️ Screenshots can capture passwords, personal messages, financial details, and other sensitive information. Keep the following in mind:

- **Avoid capturing sensitive sessions** — if you regularly handle personal, financial, or confidential information on the monitored machine, be deliberate about when the daemon is running.
- **Store images in an inconspicuous location** — the default `Pictures\self-awareness` path is readable by any process running as your user. Consider pointing `output_dir` at a less obvious directory.
- **Use an encrypted volume** — the safest option is to store screenshots inside an encrypted container (e.g. a [VeraCrypt](https://www.veracrypt.fr/) file volume). Mount it before the daemon starts and dismount it when not in use; images are unreadable to anyone without the key while the volume is locked.
- **Native security features** are not yet built in — encryption, access control, and viewer authentication are planned for a future release.

---

## Dependencies

| Crate | Purpose |
|---|---|
| [`image`](https://crates.io/crates/image) | Image encoding (WebP, JPEG, PNG) |
| [`ratatui`](https://crates.io/crates/ratatui) | Terminal UI rendering |
| [`crossterm`](https://crates.io/crates/crossterm) | Cross-platform terminal control |
| [`serde`](https://crates.io/crates/serde) + [`serde_json`](https://crates.io/crates/serde_json) | Config serialisation |
| [`chrono`](https://crates.io/crates/chrono) | Timestamp generation |
| [`anyhow`](https://crates.io/crates/anyhow) | Error handling |
| [`dirs`](https://crates.io/crates/dirs) | Platform paths (`%APPDATA%`, `Pictures`, …) |
| [`windows`](https://crates.io/crates/windows) | Win32 API (GDI capture, process management, console hiding) |

---

## License

MIT — see [LICENSE](LICENSE) for details.
