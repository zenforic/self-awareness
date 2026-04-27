use anyhow::Result;

use crate::config::{self, Config};
use crate::capture::capture_and_save;

/// Run the daemon loop: capture screenshots at the configured interval.
/// This function runs in an infinite loop until the process is terminated.
pub fn run(config: &Config) -> Result<()> {
    // Write PID file so TUI can stop us
    write_pid();

    // Ensure output directory exists
    std::fs::create_dir_all(&config.output_dir)?;

    loop {
        // Take screenshot (immediate write to disk — no data loss on shutdown)
        if let Err(e) = capture_and_save(&config.output_dir, &config.image_format) {
            log_message(&format!("Capture error: {}", e));
        }

        // Enforce disk space limit
        if let Err(e) = enforce_disk_limit(&config.output_dir, config.max_disk_mb) {
            log_message(&format!("Disk limit error: {}", e));
        }

        // Sleep for the configured interval
        std::thread::sleep(std::time::Duration::from_secs(config.interval_seconds));
    }
}

/// Stop the currently running daemon by reading its PID and terminating it.
pub fn stop_daemon() -> Result<()> {
    let pid_path = config::daemon_pid_path();
    if pid_path.exists() {
        let pid_str = std::fs::read_to_string(&pid_path)?;
        let pid: u32 = pid_str.trim().parse()?;
        kill_process(pid)?;
        let _ = std::fs::remove_file(&pid_path);
    }
    Ok(())
}

/// Check if a daemon is currently running by checking the PID file and process.
pub fn is_daemon_running() -> bool {
    let pid_path = config::daemon_pid_path();
    if !pid_path.exists() {
        return false;
    }
    if let Ok(pid_str) = std::fs::read_to_string(&pid_path) {
        if let Ok(pid) = pid_str.trim().parse::<u32>() {
            return is_process_running(pid);
        }
    }
    false
}

fn enforce_disk_limit(dir: &str, max_mb: u64) -> Result<()> {
    let max_bytes = max_mb as u64 * 1024 * 1024;
    let mut files = list_image_files(dir)?;

    // Sort by modification time (oldest first)
    files.sort_by_key(|f| {
        f.metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
    });

    let mut total_size: u64 = files
        .iter()
        .filter_map(|f| f.metadata().ok().map(|m| m.len()))
        .sum();

    // Delete oldest files until under limit
    for file in &files {
        if total_size <= max_bytes {
            break;
        }
        if let Ok(metadata) = file.metadata() {
            total_size = total_size.saturating_sub(metadata.len());
            let _ = std::fs::remove_file(file);
        }
    }

    Ok(())
}

fn list_image_files(dir: &str) -> Result<Vec<std::path::PathBuf>> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_file() {
            if let Some(ext) = path.extension() {
                let ext = ext.to_string_lossy().to_lowercase();
                if ext == "webp" || ext == "jpg" || ext == "jpeg" || ext == "png" {
                    files.push(path);
                }
            }
        }
    }
    Ok(files)
}

fn write_pid() {
    let pid_path = config::daemon_pid_path();
    std::fs::write(&pid_path, std::process::id().to_string()).ok();
}

fn log_message(msg: &str) {
    let log_path = config::daemon_log_path();
    if let Some(parent) = log_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
    let entry = format!("[{}] {}\n", timestamp, msg);
    let _ = std::fs::write(&log_path, entry);
}

#[cfg(target_os = "windows")]
fn is_process_running(pid: u32) -> bool {
    use windows::Win32::Foundation::STILL_ACTIVE;
    use windows::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    let handle = unsafe {
        OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION,
            false,
            pid,
        )
    };

    if handle.is_err() {
        return false;
    }

    let handle = handle.unwrap();
    let mut exit_code: u32 = 0;
    let result = unsafe { GetExitCodeProcess(handle, &mut exit_code) };

    result.is_ok() && exit_code == (STILL_ACTIVE.0 as u32)
}

#[cfg(target_os = "windows")]
fn kill_process(pid: u32) -> Result<()> {
    // HANDLE not needed — OpenProcess returns a handle directly
    use windows::Win32::System::Threading::{
        OpenProcess, PROCESS_TERMINATE, TerminateProcess,
    };

    let handle = unsafe {
        OpenProcess(PROCESS_TERMINATE, false, pid)?
    };

    unsafe {
        TerminateProcess(handle, 1)?;
    }

    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn is_process_running(_pid: u32) -> bool {
    false
}

#[cfg(not(target_os = "windows"))]
fn kill_process(_pid: u32) -> Result<()> {
    anyhow::bail!("Not supported on this platform")
}
