//! Window tracking service backed by the Hyprland IPC sockets.

pub mod hyprland_ipc;

use hyprland::{
    data::{Client, Clients},
    event_listener::AsyncEventListener,
    prelude::*,
};
use iced::{Subscription, futures::SinkExt, stream::channel};
use log::{debug, error};
use std::any::TypeId;
use std::time::Duration;
use tokio::sync::{OnceCell, broadcast, mpsc};
use tokio::time::sleep;

/// Snapshot of the windows currently known to the compositor.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DockWindows {
    /// All mapped windows ordered by focus history (most recent last).
    pub windows: Vec<WindowInfo>,
    /// Address of the currently focused window, if any.
    pub active: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WindowInfo {
    pub address: String,
    pub class: String,
    pub title: String,
}

#[derive(Debug, Clone)]
pub enum Message {
    Ready(DockWindows),
    Updated(DockWindows),
}

const BROADCAST_CAPACITY: usize = 64;

static BROADCASTER: OnceCell<broadcast::Sender<Message>> = OnceCell::const_new();

async fn broadcaster_subscribe() -> broadcast::Receiver<Message> {
    BROADCASTER
        .get_or_init(|| async {
            let (tx, _) = broadcast::channel(BROADCAST_CAPACITY);
            tokio::spawn(broadcaster_event_loop(tx.clone()));
            tx
        })
        .await
        .subscribe()
}

async fn broadcaster_event_loop(tx: broadcast::Sender<Message>) {
    match fetch_windows() {
        Ok(windows) => {
            let _ = tx.send(Message::Ready(windows));
        }
        Err(e) => error!("Failed to fetch initial window list: {e}"),
    }

    let mut listener = AsyncEventListener::new();

    // Coalesce rapid compositor events into a single throttled refetch.
    // Querying `/clients` mid-operation (e.g. during a drag, focus flurry or
    // workspace switch) can transiently yield a partial snapshot; debouncing
    // waits until the burst settles so we only ever read a consistent state,
    // and it also cuts IPC load on chatty events like `window_moved`.
    let (refresh_tx, mut refresh_rx) = mpsc::unbounded_channel::<()>();
    {
        let fetch_tx = tx.clone();
        tokio::spawn(async move {
            while refresh_rx.recv().await.is_some() {
                // Drain any further events that arrived in the same burst.
                while refresh_rx.try_recv().is_ok() {}
                sleep(Duration::from_millis(80)).await;
                match fetch_windows() {
                    Ok(windows) => {
                        let _ = fetch_tx.send(Message::Updated(windows));
                    }
                    Err(e) => error!("Failed to refresh window list: {e}"),
                }
            }
        });
    }

    macro_rules! add_refresh_handler {
        ($method:ident) => {
            listener.$method({
                let tx = refresh_tx.clone();
                move |_| {
                    let tx = tx.clone();
                    Box::pin(async move {
                        let _ = tx.send(());
                    })
                }
            });
        };
    }

    add_refresh_handler!(add_window_opened_handler);
    add_refresh_handler!(add_window_closed_handler);
    add_refresh_handler!(add_active_window_changed_handler);
    add_refresh_handler!(add_window_moved_handler);
    add_refresh_handler!(add_active_monitor_changed_handler);
    add_refresh_handler!(add_workspace_changed_handler);

    if let Err(e) = listener.start_listener_async().await {
        error!("Hyprland event listener failed: {e}");
    }
}

fn fetch_windows() -> anyhow::Result<DockWindows> {
    let mut clients: Vec<_> = Clients::get()?.to_vec();
    clients.sort_by_key(|c| c.focus_history_id);

    let windows: Vec<WindowInfo> = clients
        .iter()
        .map(|c| WindowInfo {
            address: c.address.to_string(),
            class: if c.class.is_empty() {
                c.initial_class.clone()
            } else {
                c.class.clone()
            },
            title: c.title.clone(),
        })
        .collect();

    let active = Client::get_active()
        .ok()
        .flatten()
        .map(|c| c.address.to_string());

    debug!("Fetched {} windows", windows.len());

    Ok(DockWindows { windows, active })
}

pub fn subscribe() -> Subscription<Message> {
    Subscription::run_with(TypeId::of::<DockWindows>(), |_| {
        channel(100, async |mut output| {
            let mut rx = broadcaster_subscribe().await;

            loop {
                match rx.recv().await {
                    Ok(message) => {
                        if output.send(message).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        log::warn!("Compositor subscriber lagged by {n} messages");
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        })
    })
}
