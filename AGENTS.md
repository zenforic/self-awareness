# Self-Awareness — Project Notes

## Overview
A stealth screen-logging daemon for monitoring PC access while away. Runs invisibly in the background, captures screenshots at configurable intervals, and provides a TUI for configuration.

## Architecture

### Modules
- **`main.rs`** — Entry point. Detects mode (daemon vs TUI) via named mutex acquisition and `--tui` flag. Hides console window in daemon mode.
- **`config.rs`** — Configuration struct (interval, disk limit, output dir, format, retention, startup persistence). Loaded/saved as JSON in `%APPDATA%\self-awareness\config.json`.
- **`capture.rs`** — Screen capture using GDI BitBlt (no nightly Rust required). Saves immediately to disk for zero data loss on shutdown.
- **`daemon.rs`** — Infinite capture loop. Writes PID file for TUI to stop it. Enforces disk space limits by deleting oldest files.
- **`tui.rs`** — Terminal UI using `ratatui` + `crossterm`. Allows editing settings, starting/stopping daemon, toggling startup persistence.
- **`startup.rs`** — Manages Windows Task Scheduler task (`SelfAwarenessStartup`) for hidden startup logging.
- **`cleanup.rs`** — Creates a scheduled task (`SelfAwarenessCleanup`) that runs a batch script daily to delete images older than the retention period — runs independently of the daemon.

### Program Modes
| Invocation | Behavior |
|---|---|
| `self-awareness.exe` | Daemon mode (if no other instance running), else shows TUI |
| `self-awareness.exe --tui` | Always shows TUI |

### Stealth Features
- Console window hidden via `GetConsoleWindow` + `ShowWindow(SW_HIDE)` in daemon mode
- No tray icon, no taskbar presence
- Only one instance runs at a time (named mutex: `SelfAwarenessDaemon`)
- Data never lost on shutdown — images written immediately to disk
- GDI BitBlt screen capture (no nightly Rust, no external dependencies)

### Startup Persistence
- Uses Windows Task Scheduler (`schtasks`) to create a task that runs the program at logon
- Program detects it's running as daemon (no mutex contention, no TUI flag) and runs silently
- Cleanup task uses `forfiles` to delete old images daily at 2 AM — independent of the daemon

## Building
```bash
cargo check          # Quick compilation check
cargo build          # Debug build (for testing)
cargo build --release  # Release build (production)
```

## Dependencies
- `image` — Image encoding (WebP, JPEG, PNG)
- `ratatui` + `crossterm` — Terminal UI
- `serde` + `serde_json` — Config serialization
- `chrono` — Timestamps
- `anyhow` — Error handling
- `dirs` — Platform paths (AppData, Pictures)
- `windows` — Windows API (console hiding, process management, GDI screen capture)

## Configuration
Stored in `%APPDATA%\self-awareness\config.json`:
```json
{
  "interval_seconds": 60,
  "max_disk_mb": 500,
  "output_dir": "C:\\Users\\...\\Pictures\\self-awareness",
  "image_format": "webp",
  "retention_days": 7,
  "start_on_boot": false
}
```

## TUI Controls
| Key | Action |
|---|---|
| Tab / Shift+Tab | Navigate fields |
| Enter | Edit current field |
| Esc | Cancel edit / Quit |
| S | Save config |
| D | Start daemon |
| X | Stop daemon |
| C | Toggle startup persistence |
| Q | Quit |

## Files
| File | Purpose |
|---|---|
| `%APPDATA%\self-awareness\config.json` | Configuration |
| `%APPDATA%\self-awareness\daemon.pid` | Daemon PID (for TUI to stop it) |
| `%APPDATA%\self-awareness\daemon.log` | Daemon error log |
| `%APPDATA%\self-awareness\cleanup.bat` | Cleanup batch script |
| Windows Task Scheduler | `SelfAwarenessStartup` and `SelfAwarenessCleanup` tasks |

## Notes
- The `screenshot` crate was initially planned but requires nightly Rust — replaced with direct GDI BitBlt via the `windows` crate
- WebP encoding uses `image` crate's built-in codec (lossy at 95% quality for small file sizes)
- The release profile enables LTO, stripping, and max optimization
