//! Configuration types and loading for s-dock.
//!
//! The config file lives at `~/.config/s-dock/config.toml` by default and is
//! hot-reloaded through an inotify watcher (see [`subscription`]).
//! Pinned applications intentionally live in a separate state file
//! (`~/.config/s-dock/pins.toml`), not here.

use iced::Subscription;
use log::{info, warn};
use serde::Deserialize;
use std::path::{Path, PathBuf};

const DEFAULT_CONFIG_FILE_PATH: &str = "~/.config/s-dock/config.toml";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Position {
    Top,
    Left,
    Right,
    #[default]
    Bottom,
}

impl Position {
    pub fn is_horizontal(self) -> bool {
        matches!(self, Position::Top | Position::Bottom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Alignment {
    Start,
    #[default]
    Center,
    End,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    #[default]
    Fixed,
    Popup,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum WidthMode {
    #[default]
    Fit,
    Full,
}

/// Where the launcher button sits inside the dock row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum LauncherPosition {
    #[default]
    Start,
    End,
}

/// Width of the popup-mode hotspot strip: either the same width as the dock
/// (`"dock"`) or a fixed number of logical pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HotspotWidth {
    #[default]
    Dock,
    Pixels(u32),
}

impl<'de> Deserialize<'de> for HotspotWidth {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct Visitor;

        impl serde::de::Visitor<'_> for Visitor {
            type Value = HotspotWidth;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("\"dock\" or a pixel count")
            }

            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Self::Value, E> {
                if v.eq_ignore_ascii_case("dock") {
                    Ok(HotspotWidth::Dock)
                } else {
                    Err(E::invalid_value(serde::de::Unexpected::Str(v), &self))
                }
            }

            fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<Self::Value, E> {
                u32::try_from(v)
                    .map(HotspotWidth::Pixels)
                    .map_err(|_| E::invalid_value(serde::de::Unexpected::Unsigned(v), &self))
            }
        }

        deserializer.deserialize_any(Visitor)
    }
}

fn default_background_color() -> hex_color::HexColor {
    hex_color::HexColor::parse("#1e1e2ecc").unwrap()
}

fn default_border_color() -> hex_color::HexColor {
    hex_color::HexColor::parse("#89b4fa00").unwrap()
}

fn default_running_indicator_color() -> hex_color::HexColor {
    hex_color::HexColor::parse("#89b4fa").unwrap()
}

fn default_launcher_text_color() -> hex_color::HexColor {
    hex_color::HexColor::parse("#cdd6f4").unwrap()
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct AppearanceConfig {
    pub icon_size: u32,
    pub dock_size: u32,
    pub width: WidthMode,
    pub margin: i32,
    pub spacing: f32,
    pub background_color: hex_color::HexColor,
    /// Extra transparency multiplier for the dock background/border (0..1).
    pub opacity: f32,
    pub border_radius: f32,
    pub border_color: hex_color::HexColor,
    pub hover_scale: f32,
    pub show_running_indicator: bool,
    pub running_indicator_color: hex_color::HexColor,
    pub running_indicator_size: f32,
}

impl Default for AppearanceConfig {
    fn default() -> Self {
        Self {
            icon_size: 40,
            dock_size: 64,
            width: WidthMode::default(),
            margin: 6,
            spacing: 8.,
            background_color: default_background_color(),
            opacity: 1.0,
            border_radius: 18.,
            border_color: default_border_color(),
            hover_scale: 1.3,
            show_running_indicator: true,
            running_indicator_color: default_running_indicator_color(),
            running_indicator_size: 5.,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct PopupConfig {
    pub hotspot_width: HotspotWidth,
    pub hotspot_height: u32,
    /// Seconds without the cursor over the dock before it hides again.
    /// `0` disables auto-hide.
    pub autohide_timeout: u64,
}

impl Default for PopupConfig {
    fn default() -> Self {
        Self {
            hotspot_width: HotspotWidth::Dock,
            hotspot_height: 8,
            autohide_timeout: 3,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct LauncherConfig {
    pub enabled: bool,
    pub label: String,
    pub cmd: String,
    /// Side of the dock row where the button is placed.
    pub position: LauncherPosition,
    pub text_color: hex_color::HexColor,
    /// Transparency multiplier for the button label (0..1).
    pub opacity: f32,
}

impl Default for LauncherConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            label: "\u{1f680}".to_string(),
            cmd: String::new(),
            position: LauncherPosition::Start,
            text_color: default_launcher_text_color(),
            opacity: 1.0,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Config {
    pub log_level: String,
    pub position: Position,
    pub alignment: Alignment,
    pub mode: Mode,
    pub appearance: AppearanceConfig,
    pub popup: PopupConfig,
    pub launcher: LauncherConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            log_level: "warn".to_owned(),
            position: Position::default(),
            alignment: Alignment::default(),
            mode: Mode::default(),
            appearance: AppearanceConfig::default(),
            popup: PopupConfig::default(),
            launcher: LauncherConfig::default(),
        }
    }
}

fn read_config(path: &Path) -> anyhow::Result<Config> {
    let content = std::fs::read_to_string(path)?;
    let config: Config = toml::from_str(&content)?;

    Ok(config)
}

fn expand_path(path: PathBuf) -> Result<PathBuf, Box<dyn std::error::Error + Send>> {
    shellexpand::path::full(&path)
        .map(|p| p.to_path_buf())
        .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send>)
}

pub fn get_config(
    path: Option<PathBuf>,
) -> Result<(Config, PathBuf), Box<dyn std::error::Error + Send>> {
    match path {
        Some(p) => {
            info!("Config path provided {p:?}");
            expand_path(p).and_then(|expanded| {
                if !expanded.exists() {
                    Err(Box::new(std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        format!("Config file does not exist: {}", expanded.display()),
                    )) as Box<dyn std::error::Error + Send>)
                } else {
                    Ok((read_config(&expanded).unwrap_or_default(), expanded))
                }
            })
        }
        None => expand_path(PathBuf::from(DEFAULT_CONFIG_FILE_PATH)).map(|expanded| {
            if expanded.exists() {
                info!("Reading config file at {expanded:?}");
                (read_config(&expanded).unwrap_or_default(), expanded)
            } else {
                warn!("Config file does not exist at {expanded:?}, using default configuration");
                (Config::default(), expanded)
            }
        }),
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    ConfigChanged(Box<Config>),
}

pub fn subscription(path: PathBuf) -> Subscription<Message> {
    Subscription::run_with(path.clone(), |key| {
        let path = std::fs::canonicalize(key).unwrap_or_else(|_| key.clone());
        iced::stream::channel(100, async move |mut output| {
            crate::utils::watch::run_watch_loop(path.clone(), &mut output, |event| match event {
                crate::utils::watch::FileEvent::Changed => {
                    Message::ConfigChanged(Box::new(read_config(&path).unwrap_or_default()))
                }
                crate::utils::watch::FileEvent::Removed => Message::ConfigChanged(Box::default()),
            })
            .await
        })
    })
}
