//! Generic inotify-based single-file watcher.
//!
//! `run_watch_loop` is an async helper so callers can build their own
//! `Subscription::run_with` channel (whose outer closure must be
//! non-capturing) while converting events into their own message types.

use inotify::{Inotify, WatchMask};
use log::{debug, error};
use std::path::PathBuf;
use std::time::Duration;

use iced::futures::SinkExt;
use iced::futures::channel::mpsc::Sender;

#[derive(Debug, Clone)]
pub enum FileEvent {
    Changed,
    Removed,
}

/// Watches `path` until the receiver disconnects, forwarding converted events.
///
/// # Panics
/// Panics if `path` has no file name or parent directory.
pub async fn run_watch_loop<M, F>(path: PathBuf, output: &mut Sender<M>, convert: F)
where
    M: Send,
    F: Fn(FileEvent) -> M,
{
    let (Some(folder), Some(file_name)) = (path.parent(), path.file_name()) else {
        error!("Cannot watch {path:?}: no file name or parent");
        return;
    };

    let Ok(inotify) = Inotify::init() else {
        error!("Failed to initialize inotify for {path:?}");
        return;
    };

    if let Err(e) = inotify.watches().add(
        folder,
        WatchMask::CREATE | WatchMask::DELETE | WatchMask::MOVE | WatchMask::MODIFY,
    ) {
        error!("Failed to add watch for {folder:?}: {e}");
        return;
    }

    let buffer = [0; 1024];
    let Ok(stream) = inotify.into_event_stream(buffer) else {
        error!("Failed to create inotify event stream");
        return;
    };

    debug!("Watching {path:?}");
    let mut events = iced::futures::StreamExt::ready_chunks(stream, 10);

    loop {
        let chunk = iced::futures::StreamExt::next(&mut events)
            .await
            .unwrap_or_default();
        let mut relevant: Option<FileEvent> = None;

        for event in chunk {
            let Ok(inotify::Event {
                name: Some(name),
                mask,
                ..
            }) = event
            else {
                continue;
            };
            if name != file_name {
                continue;
            }

            if mask.intersects(inotify::EventMask::DELETE | inotify::EventMask::MOVED_FROM) {
                relevant = Some(FileEvent::Removed);
            } else if mask.intersects(
                inotify::EventMask::CREATE
                    | inotify::EventMask::MODIFY
                    | inotify::EventMask::MOVED_TO,
            ) {
                relevant = Some(FileEvent::Changed);
            }
        }

        match relevant {
            Some(FileEvent::Removed) => {
                // Wait and double-check the file is really gone.
                tokio::time::sleep(Duration::from_millis(500)).await;
                if !path.exists() {
                    let _ = output.send(convert(FileEvent::Removed)).await;
                }
            }
            Some(changed @ FileEvent::Changed) => {
                let _ = output.send(convert(changed)).await;
            }
            None => {}
        }
    }
}
