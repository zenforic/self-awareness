mod capture;
mod config;
mod daemon;
mod startup;
mod cleanup;
mod tui;

use std::env;

/// Hide the console window on Windows.
/// This makes the daemon effectively invisible in Task Manager's "Details" tab
/// (no console window attached).
#[cfg(target_os = "windows")]
fn hide_console() {
    use windows::Win32::System::Console::GetConsoleWindow;
    use windows::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_HIDE};

    unsafe {
        let hwnd = GetConsoleWindow();
        if !hwnd.0.is_null() {
            let _ = ShowWindow(hwnd, SW_HIDE);
        }
    }
}

#[cfg(not(target_os = "windows"))]
fn hide_console() {}

/// Acquire a named mutex to ensure only one daemon instance runs.
/// Returns true if we acquired it, false if another instance holds it.
#[cfg(target_os = "windows")]
fn acquire_daemon_mutex() -> bool {
    use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError};
    use windows::Win32::System::Threading::CreateMutexA;
    use windows::core::PCSTR;

    let name = b"SelfAwarenessDaemon\0";
    let handle = unsafe {
        CreateMutexA(
            None,
            false,
            PCSTR(name.as_ptr() as *const u8),
        )
    };

    if handle.is_err() || unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        return false;
    }

    // Store handle to keep mutex alive for the duration of the process
    let _ = handle.map(|h| {
        let _ = std::sync::Mutex::new(h);
    });

    true
}

#[cfg(not(target_os = "windows"))]
fn acquire_daemon_mutex() -> bool {
    true
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let tui_mode = args.iter().any(|a| a == "--tui" || a == "-t");
    let daemon_mode = args.iter().any(|a| a == "--daemon" || a == "-d");

    // Ensure app directory exists
    let app_dir = config::app_dir();
    let _ = std::fs::create_dir_all(&app_dir);

    if daemon_mode {
        // Explicit daemon mode — run as daemon directly (no mutex check needed)
        hide_console();
        run_daemon_application();
    } else if tui_mode {
        // Always show TUI when requested
        run_tui_application();
    } else {
        // Try to acquire daemon mutex
        if acquire_daemon_mutex() {
            // We got the mutex — run as daemon
            hide_console();
            run_daemon_application();
        } else {
            // Another instance is running — show TUI
            run_tui_application();
        }
    }
}

fn run_tui_application() {
    let mut config = match config::Config::load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Failed to load config: {}", e);
            return;
        }
    };

    loop {
        let action = match tui::run_tui(&mut config) {
            Ok(a) => a,
            Err(e) => {
                eprintln!("TUI error: {}", e);
                break;
            }
        };

        match action {
            tui::TuiAction::Quit => {
                // Q: daemon already stopped by TUI, just exit
                break;
            }
            tui::TuiAction::QuitNoStop => {
                // Esc: daemon still running, just exit
                break;
            }
        }
    }
}

fn run_daemon_application() {
    let config = match config::Config::load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Failed to load config: {}", e);
            return;
        }
    };

    eprintln!("Self-Awareness daemon started (PID: {})", std::process::id());
    eprintln!(
        "  Interval: {}s | Max disk: {} MB | Format: {}",
        config.interval_seconds,
        config.max_disk_mb,
        config.image_format.label()
    );
    eprintln!(
        "  Output: {} | Retention: {} days",
        config.output_dir, config.retention_days
    );

    if let Err(e) = daemon::run(&config) {
        eprintln!("Daemon error: {}", e);
    }
}
