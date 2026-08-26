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
use tokio::sync::{OnceCell, broadcast};

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

    // The listener dies permanently on any socket error; restart it so
    // window tracking survives compositor hiccups.
    loop {
        let mut listener = AsyncEventListener::new();

        macro_rules! add_refresh_handler {
            ($method:ident) => {
                listener.$method({
                    let tx = tx.clone();
                    move |_| {
                        let tx = tx.clone();
                        Box::pin(async move {
                            match fetch_windows() {
                                Ok(windows) => {
                                    let _ = tx.send(Message::Updated(windows));
                                }
                                Err(e) => error!("Failed to refresh window list: {e}"),
                            }
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
            error!("Hyprland event listener failed: {e}; restarting in 5 s");
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        } else {
            error!("Hyprland event listener ended unexpectedly; restarting in 5 s");
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        }
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
                            debug!("Compositor subscriber disconnected");
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
