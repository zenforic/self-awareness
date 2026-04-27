use anyhow::Result;

/// Enable startup persistence by creating a Windows Task Scheduler task
/// that runs the program at logon (hidden).
pub fn enable_startup() -> Result<()> {
    let exe_path = std::env::current_exe()?.to_string_lossy().to_string();
    let task_name = "SelfAwarenessStartup";

    // Create task via schtasks directly (not through cmd /C to avoid escaping issues)
    // Pass --daemon so the startup execution bypasses mutex check and always runs as daemon
    let output = std::process::Command::new("schtasks")
        .args(&[
            "/Create",
            "/TN", task_name,
            "/TR", &format!("\"{}\" --daemon", exe_path),
            "/SC", "ONLOGON",
            "/RL", "HIGHEST",
            "/F",
        ])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Failed to create startup task: {}", stderr);
    }

    Ok(())
}

/// Disable startup persistence by removing the Task Scheduler task.
pub fn disable_startup() -> Result<()> {
    let task_name = "SelfAwarenessStartup";

    let output = std::process::Command::new("schtasks")
        .args(&["/Delete", "/TN", task_name, "/F"])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Failed to remove startup task: {}", stderr);
    }

    Ok(())
}

/// Check if startup persistence is enabled.
pub fn is_enabled() -> Result<bool> {
    let task_name = "SelfAwarenessStartup";

    let output = std::process::Command::new("schtasks")
        .args(&["/Query", "/TN", task_name, "/V", "/FO", "LIST"])
        .output()?;

    Ok(output.status.success())
}
