//! Persistent application pins stored in `~/.config/s-dock/pins.toml`.
//!
//! The file is intentionally separate from the user-edited main config:
//! pins change through the UI (and external edits are picked up live).

use log::{info, warn};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

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
    path: PathBuf,
    ids: Vec<String>,
}

impl Pins {
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

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn ids(&self) -> &[String] {
        &self.ids
    }

    /// Reloads from disk keeping the same path (used by the file watcher).
    pub fn reload_from_disk(&mut self) {
        *self = Pins::load(
            self.path
                .parent()
                .map(|p| p.join("config.toml"))
                .unwrap_or_else(|| PathBuf::from("config.toml")),
        );
    }

    pub fn contains(&self, id: &str) -> bool {
        self.ids.iter().any(|i| i == id)
    }

    /// Toggles a pin; newly pinned apps go last. Returns true if changed.
    pub fn toggle(&mut self, id: &str) -> bool {
        if let Some(pos) = self.ids.iter().position(|i| i == id) {
            self.ids.remove(pos);
        } else {
            self.ids.push(id.to_string());
        }
        self.save()
    }

    /// Moves `id` before `before_id` (used by drag-and-drop reordering).
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
