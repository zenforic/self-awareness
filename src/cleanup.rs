use anyhow::Result;

use crate::config::{self, Config};
use crate::elevate::run_schtasks_auto_elevate;

/// Create a Windows Task Scheduler task that runs a cleanup script daily
/// to delete images older than the configured retention period.
/// This runs independently of the daemon, so images are cleaned up
/// even if the program is not running.
pub fn create_cleanup_task(config: &Config) -> Result<()> {
    let app_dir = config::app_dir();
    let script_path = app_dir.join("cleanup.bat");

    // Write the cleanup batch script
    // When encryption is enabled, files are .enc; otherwise use the image format extension.
    let file_extension = if config.encrypt_images {
        crate::crypto::ENCRYPTED_EXTENSION
    } else {
        config.image_format.extension()
    };
    let batch_content = format!(
        "@echo off\nforfiles /p \"{}\" /m *.{} /d -{} /c \"cmd /c del @path\" 2>nul\n",
        config.output_dir,
        file_extension,
        config.retention_days
    );
    std::fs::write(&script_path, batch_content)?;

    // Create scheduled task to run the cleanup script daily at 2 AM
    let task_name = "SelfAwarenessCleanup";
    let script_path_str = script_path.to_string_lossy().to_string();

    run_schtasks_auto_elevate(
        "schtasks",
        &[
            "/Create",
            "/TN", task_name,
            "/TR", &format!("cmd /c \"{}\"", script_path_str),
            "/SC", "DAILY",
            "/ST", "02:00",
            "/F",
        ],
    )?;

    Ok(())
}

/// Remove the cleanup scheduled task.
pub fn remove_cleanup_task() -> Result<()> {
    let task_name = "SelfAwarenessCleanup";

    let output = std::process::Command::new("schtasks")
        .args(&["/Delete", "/TN", task_name, "/F"])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Failed to remove cleanup task: {}", stderr);
    }

    Ok(())
}

/// Check if the cleanup task exists.
pub fn is_cleanup_task_enabled() -> Result<bool> {
    let task_name = "SelfAwarenessCleanup";

    let output = std::process::Command::new("schtasks")
        .args(&["/Query", "/TN", task_name, "/V", "/FO", "LIST"])
        .output()?;

    Ok(output.status.success())
}
