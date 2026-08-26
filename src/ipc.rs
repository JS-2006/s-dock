//! s-dock IPC via Unix domain socket (`$XDG_RUNTIME_DIR/s-dock.sock`).
//! Commands: `force-visible <bool>`, `pin <id>`, `unpin <id>`, `geometry`.
//! Action commands are forwarded to the app via `Message::Ipc`; the `geometry`
//! query is answered directly from shared state (the app keeps it fresh).

use std::fmt;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::{Arc, Mutex, OnceLock};

use anyhow::{Context, Result, anyhow};
use iced::Subscription;

const MAX_REQUEST_LEN: u64 = 4096;

/// Global dock rectangle (approximation from screen size + dock size + position).
#[derive(Debug, Clone, Copy, Default)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

#[derive(Debug, Clone)]
pub enum IpcCommand {
    ForceVisible(bool),
    Pin(String),
    Unpin(String),
    Geometry,
}

impl fmt::Display for IpcCommand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IpcCommand::ForceVisible(v) => write!(f, "force-visible {v}"),
            IpcCommand::Pin(id) => write!(f, "pin {id}"),
            IpcCommand::Unpin(id) => write!(f, "unpin {id}"),
            IpcCommand::Geometry => write!(f, "geometry"),
        }
    }
}

impl FromStr for IpcCommand {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        let s = s.trim();
        if let Some(rest) = s.strip_prefix("force-visible ") {
            return Ok(IpcCommand::ForceVisible(rest.trim() == "true"));
        }
        if let Some(rest) = s.strip_prefix("pin ") {
            return Ok(IpcCommand::Pin(rest.trim().to_string()));
        }
        if let Some(rest) = s.strip_prefix("unpin ") {
            return Ok(IpcCommand::Unpin(rest.trim().to_string()));
        }
        if s == "geometry" {
            return Ok(IpcCommand::Geometry);
        }
        Err(anyhow!("unknown s-dock IPC command: {s:?}"))
    }
}

pub fn socket_path() -> Result<PathBuf> {
    if let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR") {
        return Ok(PathBuf::from(dir).join("s-dock.sock"));
    }
    let uid = unsafe { libc::getuid() };
    Ok(PathBuf::from(format!("/tmp/s-dock-{uid}.sock")))
}

enum ListenerError {
    AlreadyRunning,
    Other(anyhow::Error),
}

fn create_listener() -> std::result::Result<UnixListener, ListenerError> {
    let path = socket_path().map_err(ListenerError::Other)?;
    match UnixStream::connect(&path) {
        Ok(_) => return Err(ListenerError::AlreadyRunning),
        Err(e) if e.kind() == std::io::ErrorKind::ConnectionRefused => {
            if let Err(e) = std::fs::remove_file(&path)
                && e.kind() != std::io::ErrorKind::NotFound
            {
                return Err(ListenerError::Other(
                    anyhow::Error::new(e).context("remove stale socket"),
                ));
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            return Err(ListenerError::Other(
                anyhow::Error::new(e).context("probe socket"),
            ));
        }
    }
    let listener = UnixListener::bind(&path)
        .with_context(|| format!("bind {path:?}"))
        .map_err(ListenerError::Other)?;
    listener
        .set_nonblocking(true)
        .context("set_nonblocking")
        .map_err(ListenerError::Other)?;
    Ok(listener)
}

fn read_request(stream: &UnixStream) -> Result<IpcCommand> {
    let mut line = String::new();
    BufReader::new(stream.take(MAX_REQUEST_LEN))
        .read_line(&mut line)
        .context("read IPC command")?;
    line.trim().parse()
}

fn write_response(stream: &mut UnixStream, response: &str) {
    let msg = format!("{response}\n");
    if let Err(e) = stream.write_all(msg.as_bytes()) {
        log::debug!("s-dock IPC write response failed: {e}");
    }
}

/// Returns `Some(cmd)` for action commands (acked with "ok"), `None` for
/// `geometry` (responded directly from shared state).
fn handle_connection(mut stream: UnixStream, rect: Arc<Mutex<Option<Rect>>>) -> Option<IpcCommand> {
    let cmd = match read_request(&stream) {
        Ok(c) => c,
        Err(e) => {
            write_response(&mut stream, &format!("error {e:#}"));
            return None;
        }
    };
    match cmd {
        IpcCommand::Geometry => {
            let r = rect.lock().unwrap();
            match *r {
                Some(r) => write_response(&mut stream, &format!("{} {} {} {}", r.x, r.y, r.w, r.h)),
                None => write_response(&mut stream, "error no-geometry"),
            }
            None
        }
        other => {
            write_response(&mut stream, "ok");
            Some(other)
        }
    }
}

fn init_listener() -> Option<tokio::net::UnixListener> {
    let std_listener = match create_listener() {
        Ok(l) => l,
        Err(ListenerError::AlreadyRunning) => {
            log::warn!("another s-dock owns the IPC socket; running without IPC");
            return None;
        }
        Err(ListenerError::Other(e)) => {
            log::error!("Failed to create s-dock IPC listener: {e:#}");
            return None;
        }
    };
    match tokio::net::UnixListener::from_std(std_listener) {
        Ok(l) => Some(l),
        Err(e) => {
            log::error!("Failed to convert s-dock IPC listener to tokio: {e}");
            None
        }
    }
}

/// Shared dock-geometry handle, populated once at startup via `init`. The IPC
/// subscription closure must be non-capturing (it is registered as an `fn`
/// pointer), so it reads this global instead of capturing the `Arc`.
static SHARED_RECT: OnceLock<Arc<Mutex<Option<Rect>>>> = OnceLock::new();

/// Store the dock-geometry handle; call once before `subscription`.
pub fn init(rect: Arc<Mutex<Option<Rect>>>) {
    let _ = SHARED_RECT.set(rect);
}

pub fn subscription() -> Subscription<IpcCommand> {
    use iced::futures::StreamExt;
    Subscription::run(|| {
        let rect = SHARED_RECT
            .get()
            .expect("s-dock ipc::init not called")
            .clone();
        iced::futures::stream::unfold(None::<tokio::net::UnixListener>, move |listener| {
            let rect = rect.clone();
            async move {
                let listener = match listener {
                    Some(l) => l,
                    None => init_listener()?,
                };
                let (request, listener) = match listener.accept().await {
                    Ok((stream, _)) => {
                        let request = match stream.into_std() {
                            Ok(std_stream) => handle_connection(std_stream, rect.clone()),
                            Err(e) => {
                                log::error!("s-dock IPC stream conversion error: {e}");
                                None
                            }
                        };
                        (request, listener)
                    }
                    Err(e) => {
                        log::error!("s-dock IPC accept error: {e}");
                        (None, listener)
                    }
                };
                Some((request, Some(listener)))
            }
        })
        .filter_map(iced::futures::future::ready)
    })
}
