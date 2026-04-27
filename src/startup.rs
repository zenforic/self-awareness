use anyhow::Result;

/// Enable startup persistence by creating a Windows Task Scheduler task
/// that runs the program at logon (hidden).
pub fn enable_startup() -> Result<()> {
    let exe_path = std::env::current_exe()?.to_string_lossy().to_string();
    let task_name = "SelfAwarenessStartup";

    // Use schtasks to create a task that runs at logon
    // The program itself hides its console in daemon mode
    let command = format!(
        "schtasks /Create /TN \"{}\" /TR \"\\\"{}\\\"\" /SC ONLOGON /RL HIGHEST /F",
        task_name, exe_path
    );

    let output = std::process::Command::new("cmd")
        .args(&["/C", &command])
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

    let command = format!("schtasks /Delete /TN \"{}\" /F", task_name);

    let output = std::process::Command::new("cmd")
        .args(&["/C", &command])
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
