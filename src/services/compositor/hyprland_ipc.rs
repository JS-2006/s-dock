//! Thin client for the Hyprland IPC unix socket.
//!
//! Hyprland 0.55 replaced the hyprlang config with a Lua-based one and the
//! legacy plain-text dispatch requests (`dispatch workspace 3`) no longer
//! work: they are interpreted as Lua expressions. This module detects the
//! supported dialect once (by probing `eval`) and lets callers send either
//! Lua dispatch expressions or legacy dispatcher strings.
//!
//! Socket layout (unchanged across versions):
//! `$XDG_RUNTIME_DIR/hypr/$HYPRLAND_INSTANCE_SIGNATURE/.socket.sock`
//!
//! Requests are written as `[flags]/request args`; responses are read until
//! EOF. Per the wiki, connections must be short-lived — the compositor
//! handles them synchronously.

use anyhow::{Context, Result};
use std::path::PathBuf;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::sync::OnceCell;
use tokio::time::sleep;

/// Which dispatch dialect the running compositor understands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DispatchMode {
    /// Hyprland >= 0.55 (Lua config): dispatches go through `hl.dsp.*`.
    Lua,
    /// Legacy hyprlang config: plain-text dispatcher syntax.
    Legacy,
}

static DISPATCH_MODE: OnceCell<DispatchMode> = OnceCell::const_new();

fn socket_path() -> Result<PathBuf> {
    let instance = std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE")
        .ok_or_else(|| anyhow::anyhow!("HYPRLAND_INSTANCE_SIGNATURE is not set"))?;
    let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/run/user").join(unsafe { libc::getuid() }.to_string()));
    Ok(runtime_dir.join("hypr").join(instance).join(".socket.sock"))
}

/// Sends a raw request and returns the compositor's response.
///
/// Retries transient socket errors: Hyprland services IPC synchronously and
/// can briefly refuse or reset a connection under load (e.g. right as a
/// window starts closing), which otherwise surfaces as a "failed writing to
/// the Hyprland IPC socket" error.
async fn send_request(request: &str) -> Result<String> {
    let path = socket_path()?;
    let mut last_err: Option<anyhow::Error> = None;

    for _ in 0..3 {
        let mut stream = match UnixStream::connect(&path).await {
            Ok(s) => s,
            Err(e) => {
                last_err = Some(
                    anyhow::anyhow!(e).context("failed connecting to the Hyprland IPC socket"),
                );
                sleep(Duration::from_millis(40)).await;
                continue;
            }
        };

        if let Err(e) = write_request(&mut stream, request).await {
            last_err = Some(e);
            sleep(Duration::from_millis(40)).await;
            continue;
        }

        let mut response = String::new();
        match stream.read_to_string(&mut response).await {
            Ok(_) => return Ok(response),
            Err(e) => {
                last_err =
                    Some(anyhow::anyhow!(e).context("failed reading from the Hyprland IPC socket"));
                sleep(Duration::from_millis(40)).await;
                continue;
            }
        }
    }

    Err(last_err
        .unwrap_or_else(|| anyhow::anyhow!("failed communicating with the Hyprland IPC socket")))
}

/// Writes the request line (plus trailing newline) to the socket.
async fn write_request(stream: &mut UnixStream, request: &str) -> Result<()> {
    stream
        .write_all(request.as_bytes())
        .await
        .context("failed writing to the Hyprland IPC socket")?;
    stream
        .write_all(b"\n")
        .await
        .context("failed writing to the Hyprland IPC socket")?;
    Ok(())
}

/// Sends a request and maps error-looking responses into `Err`.
pub async fn send_and_check(request: &str) -> Result<()> {
    let response = send_request(request).await?;
    let trimmed = response.trim_end();
    if trimmed.is_empty() || trimmed.starts_with("ok") {
        Ok(())
    } else {
        Err(anyhow::anyhow!("Hyprland IPC error: {}", trimmed))
    }
}

async fn detect_mode() -> DispatchMode {
    match send_request("/eval return 1").await {
        Ok(response) if response.starts_with("ok") => {
            log::debug!("Hyprland runs with the Lua config manager");
            DispatchMode::Lua
        }
        _ => {
            log::debug!("Hyprland runs with the legacy (hyprlang) config manager");
            DispatchMode::Legacy
        }
    }
}

/// Returns the detected dispatch dialect, probing the compositor only once.
pub async fn dispatch_mode() -> DispatchMode {
    *DISPATCH_MODE.get_or_init(detect_mode).await
}

/// Evaluates a Lua expression that performs a dispatch, e.g.
/// `hl.dsp.focus({ workspace = "3" })`.
pub async fn eval_dispatch(lua_expr: &str) -> Result<()> {
    send_and_check(&format!("/eval return hl.dispatch({})", lua_expr)).await
}

/// Sends a legacy plain-text dispatcher request, e.g. `workspace 3`.
pub async fn legacy_dispatch(dispatcher_args: &str) -> Result<()> {
    send_and_check(&format!("/dispatch {}", dispatcher_args)).await
}
