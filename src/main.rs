mod capture;
mod cleanup;
mod config;
mod crypto;
mod daemon;
mod elevate;
mod startup;
mod tui;
mod viewer;

use std::env;

/// Hide the console window on Windows.
/// This makes the daemon effectively invisible in Task Manager's "Details" tab
/// (no console window attached).
#[cfg(target_os = "windows")]
fn hide_console() {
    use windows::Win32::System::Console::GetConsoleWindow;
    use windows::Win32::UI::WindowsAndMessaging::{SW_HIDE, ShowWindow};

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
    let handle = unsafe { CreateMutexA(None, false, PCSTR(name.as_ptr() as *const u8)) };

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

    // Handle setting a passphrase via CLI
    let set_pass = args.iter().any(|a| a == "--set-passphrase");
    if set_pass {
        let needs = crypto::needs_passphrase().unwrap_or(false);
        let old_pass = if needs {
            Some(rpassword::prompt_password("Enter current passphrase: ").unwrap_or_default())
        } else {
            None
        };
        let new_pass = rpassword::prompt_password("Enter NEW passphrase (empty to disable): ")
            .unwrap_or_default();
        let new_pass_opt = if new_pass.trim().is_empty() {
            None
        } else {
            Some(new_pass.as_str())
        };

        let old_pass_opt = old_pass.as_deref();
        match crypto::set_passphrase(old_pass_opt, new_pass_opt) {
            Ok(_) => println!("Master key passphrase updated successfully."),
            Err(e) => eprintln!("Failed to update master key passphrase: {}", e),
        }
        return;
    }

    let set_tui_pass = args.iter().any(|a| a == "--set-tui-passphrase");
    if set_tui_pass {
        let mut config = config::Config::load().unwrap_or_default();
        if config.tui_passphrase_hash.is_some() {
            let old_pass = rpassword::prompt_password("Enter current TUI password: ").unwrap_or_default();
            if !crypto::verify_tui_password(&old_pass, config.tui_passphrase_hash.as_ref().unwrap()).unwrap_or(false) {
                eprintln!("Incorrect current TUI password.");
                return;
            }
        }
        let new_pass = rpassword::prompt_password("Enter NEW TUI password (empty to disable): ").unwrap_or_default();
        if new_pass.trim().is_empty() {
            config.tui_passphrase_hash = None;
            println!("TUI login password disabled.");
        } else {
            let confirm = rpassword::prompt_password("Confirm NEW TUI password: ").unwrap_or_default();
            if new_pass != confirm {
                eprintln!("Passwords do not match.");
                return;
            }
            config.tui_passphrase_hash = Some(crypto::hash_tui_password(&new_pass).expect("Failed to hash"));
            println!("TUI login password updated successfully.");
        }
        config.save().expect("Failed to save config");
        return;
    }

    let tui_mode = args.iter().any(|a| a == "--tui" || a == "-t");
    let daemon_mode = args.iter().any(|a| a == "--daemon" || a == "-d");

    // Ensure app directory exists
    let app_dir = config::app_dir();
    let _ = std::fs::create_dir_all(&app_dir);

    // Retrieve passphrase from env, or prompt if needed in TUI mode
    let env_pass = std::env::var("SAW_PASSPHRASE").ok();

    if daemon_mode {
        // Explicit daemon mode — run as daemon directly (no mutex check needed)
        hide_console();
        run_daemon_application(env_pass);
    } else if tui_mode {
        // Always show TUI when requested
        run_tui_wrapper(env_pass);
    } else {
        // Normal startup: check PID file first to detect running daemon
        let daemon_running = daemon::is_daemon_running();
        if daemon_running {
            // A daemon is already running — show TUI to manage it
            run_tui_wrapper(env_pass);
        } else {
            // No running daemon — try to acquire mutex and start one
            if acquire_daemon_mutex() {
                hide_console();
                run_daemon_application(env_pass);
            } else {
                // Mutex is held, but PID file is missing or stale.
                // This means a zombie daemon is running! Kill it and try again.
                let _ = daemon::stop_daemon();

                // Sleep briefly to let the zombie release the mutex
                std::thread::sleep(std::time::Duration::from_millis(200));

                if acquire_daemon_mutex() {
                    hide_console();
                    run_daemon_application(env_pass);
                } else {
                    // Another instance is running and couldn't be killed — show TUI
                    run_tui_wrapper(env_pass);
                }
            }
        }
    }
}

fn run_tui_wrapper(env_pass: Option<String>) {
    let config = match config::Config::load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Failed to load config: {}", e);
            return;
        }
    };
    
    if let Some(hash) = &config.tui_passphrase_hash {
        let mut attempts = 0;
        loop {
            if let Ok(p) = rpassword::prompt_password("Enter TUI login password: ") {
                if crypto::verify_tui_password(&p, hash).unwrap_or(false) {
                    break;
                } else {
                    eprintln!("Incorrect password.");
                    attempts += 1;
                    if attempts >= 3 {
                        return;
                    }
                }
            } else {
                return;
            }
        }
    }

    let mut pass = env_pass;
    if pass.is_none() && crypto::needs_passphrase().unwrap_or(false) {
        if let Ok(p) = rpassword::prompt_password("Enter master key passphrase to unlock images: ") {
            // Verify passphrase before launching TUI
            if let Err(e) = crypto::load_key(Some(&p)) {
                eprintln!("Invalid passphrase or key error: {}", e);
                return;
            }
            pass = Some(p);
        } else {
            eprintln!("Failed to read passphrase.");
            return;
        }
    }
    run_tui_application(pass);
}

fn run_tui_application(passphrase: Option<String>) {
    let mut config = match config::Config::load() {
        Ok(mut c) => {
            c.current_passphrase = passphrase;
            c
        }
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

fn run_daemon_application(passphrase: Option<String>) {
    let config = match config::Config::load() {
        Ok(mut c) => {
            c.current_passphrase = passphrase;
            c
        }
        Err(e) => {
            eprintln!("Failed to load config: {}", e);
            return;
        }
    };

    eprintln!(
        "Self-Awareness daemon started (PID: {})",
        std::process::id()
    );
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
