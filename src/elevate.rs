use anyhow::Result;

/// Run a schtasks command with automatic elevation if "Access is denied".
/// This tries the command normally first, then re-spawns with UAC prompt on failure.
pub fn run_schtasks_auto_elevate(
    cmd: &str,
    args: &[&str],
) -> Result<String> {
    let output = std::process::Command::new(cmd)
        .args(args)
        .output()?;

    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).to_string());
    }

    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    // If access denied, retry with elevation via ShellExecute "runas"
    if stderr.contains("Access is denied") || stderr.contains("access denied") {
        let schtasks_path = "schtasks.exe";

        // Build the schtasks command line for ShellExecute
        let elevated_cmd: Vec<String> = args.iter().map(|a| {
            if a.contains(' ') || a.contains('"') {
                format!("\"{}\"", a.replace('"', "\"\""))
            } else {
                a.to_string()
            }
        }).collect();
        let elevated_cmd_str = format!("{} {}", schtasks_path, elevated_cmd.join(" "));

        // Encode strings to UTF-16 for ShellExecuteW
        let operation: Vec<u16> = "runas\0".encode_utf16().collect();
        let file: Vec<u16> = schtasks_path.encode_utf16().collect();
        let params: Vec<u16> = elevated_cmd_str
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();

        let result = unsafe {
            windows::Win32::UI::Shell::ShellExecuteW(
                None,
                windows::core::PCWSTR(operation.as_ptr()),
                windows::core::PCWSTR(file.as_ptr()),
                windows::core::PCWSTR(params.as_ptr()),
                None,
                windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL,
            )
        };

        if result.0 as isize > 32 {
            // ShellExecute launched the elevated process.
            // Since it's fire-and-forget, we wait briefly then retry
            // the command (now running elevated).
            std::thread::sleep(std::time::Duration::from_millis(1500));

            let output2 = std::process::Command::new(cmd)
                .args(args)
                .output()?;

            if output2.status.success() {
                return Ok(String::from_utf8_lossy(&output2.stdout).to_string());
            }

            let stderr2 = String::from_utf8_lossy(&output2.stderr).to_string();
            anyhow::bail!("Failed to create task (elevated): {}", stderr2);
        } else {
            // User cancelled UAC prompt (error code <= 32)
            anyhow::bail!("Elevation required to create scheduled task. Operation cancelled.");
        }
    }

    anyhow::bail!("Failed to create task: {}", stderr);
}
