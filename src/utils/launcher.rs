use log::error;

/// Runs a shell command asynchronously, expanding `~` and env vars.
pub async fn execute(cmd: String) {
    let expanded = shellexpand::full(&cmd)
        .map(|c| c.to_string())
        .unwrap_or(cmd);

    match tokio::process::Command::new("sh")
        .arg("-c")
        .arg(&expanded)
        .spawn()
    {
        Ok(mut child) => {
            let _ = child.wait().await;
        }
        Err(e) => error!("Failed to run '{expanded}': {e}"),
    }
}
