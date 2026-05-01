# Self-Awareness — Project Notes

## Overview
A stealth screen-logging daemon for monitoring PC access while away. Runs invisibly in the background, captures screenshots at configurable intervals, and provides a TUI for configuration.

## Architecture

### Modules
- **`main.rs`** — Entry point. Detects mode (daemon vs TUI) via PID file check and named mutex acquisition. `--tui` flag forces TUI, `--daemon` flag forces daemon (bypasses all checks). Hides console window in daemon mode.
- **`config.rs`** — Configuration struct (interval, disk limit, output dir, format, retention, startup persistence). Loaded/saved as JSON in `%APPDATA%\self-awareness\config.json`.
- **`capture.rs`** — Screen capture using GDI BitBlt (no nightly Rust required). Saves immediately to disk for zero data loss on shutdown.
- **`daemon.rs`** — Infinite capture loop. Writes PID file for TUI to stop it. Enforces disk space limits by deleting oldest files.
- **`tui.rs`** — Terminal UI using `ratatui` + `crossterm`. Allows editing settings, starting/stopping daemon, toggling startup persistence.
- **`startup.rs`** — Manages Windows Task Scheduler task (`SelfAwarenessStartup`) for hidden startup logging.
- **`cleanup.rs`** — Creates a scheduled task (`SelfAwarenessCleanup`) that runs a batch script daily to delete images older than the retention period — runs independently of the daemon.
- **`crypto.rs`** — Handles AES-GCM encryption with DPAPI keys and manages the hash chain sequence.
- **`viewer.rs`** — Provides logic for the TUI Viewer page to list, verify, and decrypt captured images.

### Program Modes
| Invocation | Behavior |
|---|---|
| `self-awareness.exe` | Checks PID file: if daemon running → TUI (re-attach); if PID stale → TUI (stopped); else tries mutex → daemon or TUI |
| `self-awareness.exe --tui` | Always shows TUI (reattaches if daemon running) |
| `self-awareness.exe --daemon` | Always runs as daemon (bypasses all checks) |
| `self-awareness.exe --set-passphrase` | CLI prompt to set, change, or remove the optional passphrase protecting the master key |
| `self-awareness.exe --set-tui-passphrase` | CLI prompt to set a TUI-only login password |

### Startup Flow
| Scenario | Behavior |
|---|---|
| Fresh start (no PID file) | Acquires mutex → runs as daemon |
| Daemon running (PID valid) | Opens TUI in **re-attached** mode — can manage daemon |
| Stale PID (process dead) | Opens TUI showing **Stopped (Died)** — user can press D to start |
| Another instance running | Mutex fails → shows TUI |

### Stealth Features
- Console window hidden via `GetConsoleWindow` + `ShowWindow(SW_HIDE)` in daemon mode
- No tray icon, no taskbar presence
- Only one instance runs at a time (named mutex: `SelfAwarenessDaemon`)
- Data never lost on shutdown — images written immediately to disk
- GDI BitBlt screen capture (no nightly Rust, no external dependencies)
- If a passphrase is set via `--set-passphrase`, the background daemon (started at boot) will gracefully fail to start without hanging or creating windows, honoring the stealth configuration constraints.

### Startup Persistence
- Uses Windows Task Scheduler (`schtasks /Create`) to create `SelfAwarenessStartup` task at logon
- Task runs `self-awareness.exe --daemon` (bypasses mutex check for reliable startup)
- Console is hidden via `GetConsoleWindow` + `ShowWindow(SW_HIDE)` in daemon mode
- Cleanup task uses `forfiles` to delete old images daily at 2 AM — independent of the daemon
- `schtasks` is invoked directly (not via `cmd /C`) to avoid argument escaping issues
- `cleanup.bat` is regenerated when config is saved and daemon is restarted, using absolute paths
- Task runs with user-level privileges (no `/RL HIGHEST`) — sufficient for screen logging

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
- `windows` — Windows API (console hiding, process management, GDI screen capture, DPAPI)
- `aes-gcm` — Encryption at rest
- `sha2` — Tamper detection hash chain
- `rand` — Secure random generation
- `argon2` — Passphrase key derivation
- `rpassword` — Secure CLI password prompting

## Configuration
Stored in `%APPDATA%\self-awareness\config.json`:
```json
{
  "interval_seconds": 60,
  "max_disk_mb": 500,
  "output_dir": "C:\\Users\\...\\Pictures\\self-awareness",
  "image_format": "webp",
  "retention_days": 7,
  "start_on_boot": false,
  "encrypt_images": true,
  "hash_chain": true,
  "tui_passphrase_hash": null
}
```

## TUI Controls
| Key | Action |
|---|---|
| Tab / Shift+Tab | Navigate fields (or next/prev field while editing) |
| Enter | Edit current field / Confirm edit |
| Esc | Cancel edit / Quit (without stopping daemon) |
| S | Save config (settings take effect only after save) |
| D | Start daemon (stays in TUI, does not auto-save) |
| X | Stop daemon (stays in TUI, does not auto-save) |
| C | Toggle startup persistence (stays in TUI, does not auto-save) |
| T | Switch to Tasks page |
| Q | Quit (stops daemon and exits) |
| E | Exit TUI (keeps daemon running) |

### Tasks Page (T key to enter, Esc/T to return)
| Key | Action |
|---|---|
| S | Toggle SelfAwarenessStartup task |
| C | Toggle SelfAwarenessCleanup task |
| A | Clear ALL tasks (disable both) |
| T / Esc | Back to main page |
| Q | Quit (stops daemon and exits) |
| E | Exit (keeps daemon running) |

### Viewer Page (V key to enter, Esc/V to return)
| Key | Action |
|---|---|
| Up / Down | Navigate list |
| PgUp / PgDn | Fast navigation |
| F / / | Focus search filter (type to filter, Enter/Esc to finish) |
| Enter | Decrypt and open selected image in default viewer |
| I | Investigate all (decrypts all current images into a folder) |
| V / Esc | Back to main page |
| Q | Quit (stops daemon and exits) |
| E | Exit (keeps daemon running) |

### Key behaviors
- **Focused field always highlighted**: Yellow for selected, green while editing
- **Live editing**: Changes appear in the display as you type; only applied on Enter
- **Settings not auto-saved**: D, X, C, Q, and Esc do not save — only S saves
- **Commands blocked during edit**: While editing a field, only Enter/Esc/Tab/Backspace/chars work
- **Esc**: Cancels edit if editing; quits TUI without stopping the daemon if not editing
- **Q**: Stops daemon (if running) and exits TUI
- **D**: Starts daemon only if not already running (prevents multiple instances)
- **X**: Stops daemon only if running (no-op otherwise)
- **D/X/C**: Stay in TUI after action (no save, no exit)
- **Daemon messages**: Start/stop/status messages appear in the message area (below buttons) for 5 seconds instead of stderr
- **Restart prompt**: After saving config with S, TUI asks "Restart daemon? [Y/N]" — Y stops old daemon, regenerates cleanup.bat, waits 500ms, starts new daemon; status shows "Restarting..." in yellow during the process; N or Esc cancels

## Files
| File | Purpose |
|---|---|
| `%APPDATA%\self-awareness\config.json` | Configuration |
| `%APPDATA%\self-awareness\daemon.pid` | Daemon PID (for TUI to stop it) |
| `%APPDATA%\self-awareness\daemon.log` | Daemon error log |
| `%APPDATA%\self-awareness\cleanup.bat` | Cleanup batch script |
| `%APPDATA%\self-awareness\key.bin` | Master key. If no passphrase, contains `DPAPI(AES_KEY)`. If passphrase set, contains `DPAPI(MAGIC + SALT + NONCE + AES_GCM(DERIVED_KEY, AES_KEY) + TAG)`. |
| Windows Task Scheduler | `SelfAwarenessStartup` and `SelfAwarenessCleanup` tasks |

## Git Workflow
- **`main`** — Stable, tested releases. Only merge here after testing on `dev` is complete.
- **`dev`** — Active development branch. All work is done here first.
- **Note**: Git is already configured and ready for commits — no initial setup required.
- **Workflow**:
  1. Create new feature/fix branches from `dev` (e.g., `git checkout dev && git checkout -b feature-name`)
  2. Make commits on the feature branch
  3. Test thoroughly on `dev`
  4. When ready, merge feature branch into `dev`, then merge `dev` into `main`
- **Commit convention**: Use conventional commits (`fix:`, `feat:`, `refactor:`, `docs:`, `chore:`)

## Notes
- The `screenshot` crate was initially planned but requires nightly Rust — replaced with direct GDI BitBlt via the `windows` crate
- WebP encoding uses `image` crate's built-in codec (lossy at 95% quality for small file sizes)
- The release profile enables LTO, stripping, and max optimization
- Daemon is spawned from TUI with `DETACHED_PROCESS` flag so it runs independently of the TUI's console — exiting the TUI does not affect the daemon
- Normal startup checks PID file first: if a valid daemon is running, launches TUI to manage it rather than trying to start a new daemon
- Stale PID files (dead process) are cleaned up automatically; TUI shows "Stopped (Died)" so the user can restart with 'd'
- **Daemon status** shows "Running" (green), "Restarting..." (yellow, during restart), "Stopped (Died)" (yellow), or "Stopped" (red)
- **Relative paths** entered for output_dir are automatically converted to absolute paths on save
