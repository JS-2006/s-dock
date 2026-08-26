//! Generic inotify-based single-file watcher.

use inotify::{Inotify, WatchMask};
use log::{debug, error};
use std::path::PathBuf;
use std::time::Duration;

use iced::{Subscription, futures::SinkExt, stream::channel};

#[derive(Debug, Clone)]
pub enum FileEvent {
    Changed,
    Removed,
}

pub fn watch(path: PathBuf) -> Subscription<FileEvent> {
    Subscription::run_with(path.clone(), move |key| {
        let path = std::fs::canonicalize(key).unwrap_or_else(|_| key.clone());
        channel(100, async move |mut output| {
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
                    match event {
                        Ok(inotify::Event {
                            name: Some(name),
                            mask: inotify::EventMask::DELETE | inotify::EventMask::MOVED_FROM,
                            ..
                        }) if name == file_name => relevant = Some(FileEvent::Removed),
                        Ok(inotify::Event {
                            name: Some(name),
                            mask:
                                inotify::EventMask::CREATE
                                | inotify::EventMask::MODIFY
                                | inotify::EventMask::MOVED_TO,
                            ..
                        }) if name == file_name => relevant = Some(FileEvent::Changed),
                        _ => {}
                    }
                }

                match relevant {
                    Some(FileEvent::Changed) => {
                        let _ = output.send(FileEvent::Changed).await;
                    }
                    Some(FileEvent::Removed) => {
                        // Wait and double-check the file is really gone.
                        tokio::time::sleep(Duration::from_millis(500)).await;
                        if !path.exists() {
                            let _ = output.send(FileEvent::Removed).await;
                        }
                    }
                    None => {}
                }
            }
        })
    })
}
