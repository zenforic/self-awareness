use anyhow::Result;

use crate::config::{self, Config};

/// Run the daemon loop: capture screenshots at the configured interval.
/// This function runs in an infinite loop until the process is terminated.
pub fn run(config: &Config) -> Result<()> {
    // Write PID file so TUI can stop us
    write_pid();

    // Ensure output directory exists
    std::fs::create_dir_all(&config.output_dir)?;

    let mut prev_chain_hash = if config.encrypt_images && config.hash_chain {
        Some(crate::crypto::get_latest_chain_hash(&config.output_dir).unwrap_or_else(|_| {
            use sha2::Digest;
            let mut hasher = sha2::Sha256::new();
            hasher.update(b"self-awareness-genesis");
            hasher.finalize().into()
        }))
    } else {
        None
    };

    loop {
        // Take screenshot (immediate write to disk — no data loss on shutdown)
        if let Err(e) = crate::capture::capture_and_save(
            &config.output_dir,
            &config.image_format,
            config.encrypt_images,
            config.hash_chain,
            prev_chain_hash.as_mut(),
        ) {
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
    let mut killed = false;

    if pid_path.exists() {
        if let Ok(pid_str) = std::fs::read_to_string(&pid_path) {
            if let Ok(pid) = pid_str.trim().parse::<u32>() {
                if is_process_running(pid) {
                    if kill_process(pid).is_ok() {
                        killed = true;
                    }
                }
            }
        }
        // Always clean up the PID file, even if kill failed
        let _ = std::fs::remove_file(&pid_path);
    }

    // Fallback: if we didn't kill via PID, try to kill by process name
    // (handles cases where PID file was stale or missing)
    if !killed {
        let _ = kill_self_awareness_daemons();
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
                if ext == "webp" || ext == "jpg" || ext == "jpeg" || ext == "png" || ext == "enc" {
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

/// Kill all self-awareness.exe processes running as daemon (identified by --daemon flag).
/// This is a fallback for when the PID file is stale or missing.
/// Excludes the current process (the TUI) from being killed.
#[cfg(target_os = "windows")]
fn kill_self_awareness_daemons() -> Result<()> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };

    let current_pid = std::process::id();
    let snapshot = match unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) } {
        Ok(h) => h,
        Err(_) => return Ok(()),
    };

    let mut entry: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
    entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;

    // First pass: find all self-awareness.exe PIDs
    let mut targets: Vec<u32> = Vec::new();

    let mut found = unsafe { Process32FirstW(snapshot, &mut entry) };
    while found.is_ok() {
        let exe_name = entry
            .szExeFile
            .iter()
            .take_while(|&&c| c != 0)
            .map(|&c| c as u8 as char)
            .collect::<String>();

        if exe_name.to_lowercase() == "self-awareness.exe" && entry.th32ProcessID != current_pid
        {
            targets.push(entry.th32ProcessID);
        }

        found = unsafe { Process32NextW(snapshot, &mut entry) };
    }

    // Second pass: kill each target
    for pid in targets {
        let _ = kill_process(pid);
    }

    // Close the snapshot handle
    let _ = unsafe { CloseHandle(snapshot) };

    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn kill_self_awareness_daemons() -> Result<()> {
    Ok(())
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
