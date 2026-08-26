//! Persistent application pins stored in `~/.config/s-dock/pins.toml`.
//!
//! The file is intentionally separate from the user-edited main config:
//! pins change through the UI (and external edits are picked up live).

use log::{info, warn};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use iced::Subscription;

const FILE_NAME: &str = "pins.toml";

/// Derives the pins file path next to the main config file.
pub fn path_for(config_path: &Path) -> PathBuf {
    config_path
        .parent()
        .unwrap_or(Path::new("."))
        .join(FILE_NAME)
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct PinsFile {
    #[serde(default)]
    pins: Vec<String>,
}

/// Ordered list of pinned application ids (`*.desktop`).
#[derive(Debug, Clone, Default)]
pub struct Pins {
    /// Kept for `save`/`toggle` once the context menu lands.
    #[allow(dead_code)]
    path: PathBuf,
    ids: Vec<String>,
}

impl Pins {
    /// Loads pins; also the re-entry point for external file edits.
    pub fn load(config_path: &Path) -> Self {
        let path = path_for(config_path);

        let ids = match std::fs::read_to_string(&path) {
            Ok(content) => match toml::from_str::<PinsFile>(&content) {
                Ok(file) => file.pins,
                Err(e) => {
                    warn!("Failed to parse {}: {e}", path.display());
                    Vec::new()
                }
            },
            Err(_) => Vec::new(),
        };

        info!("Loaded {} pins from {}", ids.len(), path.display());

        Self { path, ids }
    }

    pub fn ids(&self) -> &[String] {
        &self.ids
    }

    pub fn contains(&self, id: &str) -> bool {
        self.ids.iter().any(|i| i == id)
    }

    /// Moves `id` to the position of `before_id` (drag-and-drop reorder).
    pub fn move_before(&mut self, id: &str, before_id: &str) -> bool {
        let Some(from) = self.ids.iter().position(|i| i == id) else {
            return false;
        };
        let entry = self.ids.remove(from);
        let target = self
            .ids
            .iter()
            .position(|i| i == before_id)
            .unwrap_or(self.ids.len());
        self.ids.insert(target, entry);
        self.save()
    }

    /// Toggles a pin; newly pinned apps go last. Returns true if changed.
    #[allow(dead_code)]
    pub fn toggle(&mut self, id: &str) -> bool {
        if let Some(pos) = self.ids.iter().position(|i| i == id) {
            self.ids.remove(pos);
        } else {
            self.ids.push(id.to_string());
        }
        self.save()
    }

    #[allow(dead_code)]
    pub fn save(&self) -> bool {
        let content = match toml::to_string_pretty(&PinsFile {
            pins: self.ids.clone(),
        }) {
            Ok(c) => c,
            Err(e) => {
                warn!("Failed to serialize pins: {e}");
                return false;
            }
        };

        // Atomic write: temp file + rename.
        let tmp = self.path.with_extension("toml.tmp");
        if let Err(e) = std::fs::write(&tmp, content) {
            warn!("Failed to write {}: {e}", tmp.display());
            return false;
        }
        if let Err(e) = std::fs::rename(&tmp, &self.path) {
            warn!("Failed to rename into {}: {e}", self.path.display());
            return false;
        }

        true
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    PinsChanged(Pins),
}

/// Watches the pins file so external edits are picked up live.
pub fn subscription(config_path: PathBuf) -> Subscription<Message> {
    Subscription::run_with(config_path.clone(), move |key| {
        let watched = crate::state::path_for(key);
        let watched = std::fs::canonicalize(&watched).unwrap_or(watched);
        let config_path = key.to_path_buf();
        iced::stream::channel(100, async move |mut output| {
            crate::utils::watch::run_watch_loop(watched, &mut output, move |_| {
                Message::PinsChanged(Pins::load(&config_path))
            })
            .await
        })
    })
}
