use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;

use iced::{
    Element, Length, OutputEvent, Subscription, SurfaceId, Task, output_events,
    widget::{button, column, container, image, row, text},
};

use crate::{
    config,
    outputs::Outputs,
    services::{
        apps::{AppRegistry, DesktopApp},
        compositor::{self, DockWindows},
    },
    theme::use_theme,
    utils::launcher,
};

pub struct App {
    config: config::Config,
    config_path: PathBuf,
    outputs: Outputs,
    registry: AppRegistry,
    windows: DockWindows,
    /// icon name → resolved path cache
    icon_cache: RefCell<HashMap<String, Option<PathBuf>>>,
}

#[derive(Debug, Clone)]
pub enum Message {
    ConfigChanged(Box<config::Config>),
    OutputEvent(OutputEvent),
    WindowsReady(DockWindows),
    WindowsUpdated(DockWindows),
    IconClicked(String),
    LaunchLauncher,
    None,
}

/// A dock entry: one running application (grouped by window class).
struct AppGroup {
    class: String,
    app: Option<DesktopApp>,
    windows: Vec<compositor::WindowInfo>,
}

impl App {
    pub fn new((config, config_path): (config::Config, PathBuf)) -> Self {
        Self {
            config,
            config_path,
            outputs: Outputs::default(),
            registry: AppRegistry::load(),
            windows: DockWindows::default(),
            icon_cache: RefCell::new(HashMap::new()),
        }
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::ConfigChanged(config) => {
                info!("Config reloaded");
                self.config = *config;
                crate::theme::update(&self.config.appearance);

                // Geometry-affecting options require fresh surfaces.
                self.outputs
                    .recreate_all(&self.config)
                    .map(|_| Message::None)
            }
            Message::OutputEvent(event) => match event {
                OutputEvent::Added(info) => self
                    .outputs
                    .create(&self.config, info.name.clone(), info.id)
                    .map(|_| Message::None),
                OutputEvent::Removed(output_id) => self
                    .outputs
                    .remove(&self.config, output_id)
                    .map(|_| Message::None),
                OutputEvent::InfoChanged(_) => Task::none(),
            },
            Message::WindowsReady(windows) | Message::WindowsUpdated(windows) => {
                self.windows = windows;
                Task::none()
            }
            Message::IconClicked(class) => self.on_icon_clicked(class),
            Message::LaunchLauncher => {
                let cmd = self.config.launcher.cmd.clone();
                if !cmd.is_empty() {
                    tokio::spawn(async move { launcher::execute(cmd).await });
                }
                Task::none()
            }
            Message::None => Task::none(),
        }
    }

    fn on_icon_clicked(&mut self, class: String) -> Task<Message> {
        let windows: Vec<compositor::WindowInfo> = self
            .windows
            .windows
            .iter()
            .filter(|w| w.class.eq_ignore_ascii_case(&class))
            .cloned()
            .collect();

        if let Some(target) = pick_focus_target(&windows, &self.windows.active) {
            let address = target.address.clone();
            tokio::spawn(async move { focus_window(&address).await });
            return Task::none();
        }

        // Not running: launch a new instance.
        if let Some(app) = self.registry.by_class(&class).cloned() {
            if !app.exec.is_empty() {
                tokio::spawn(async move { launcher::execute(app.exec).await });
            } else {
                warn!("No exec for '{}' ({class})", app.name);
            }
        }

        Task::none()
    }

    fn groups(&self) -> Vec<AppGroup> {
        let mut groups: Vec<AppGroup> = Vec::new();
        for window in &self.windows.windows {
            if let Some(group) = groups
                .iter_mut()
                .find(|g| g.class.eq_ignore_ascii_case(&window.class))
            {
                group.windows.push(window.clone());
            } else {
                groups.push(AppGroup {
                    class: window.class.clone(),
                    app: self.registry.by_class(&window.class).cloned(),
                    windows: vec![window.clone()],
                });
            }
        }
        groups
    }

    pub fn view(&self, id: SurfaceId) -> Element<'_, Message> {
        if !self.outputs.is_dock(id) {
            return row![].into();
        }

        let appearance = use_theme(|a| a.clone());
        let launcher = self.config.launcher.clone();

        // --- Launcher button ---
        let launcher_button = launcher.enabled.then(|| {
            let text_color = color_from_hex(launcher.text_color, launcher.opacity);
            button(
                text(launcher.label)
                    .size(appearance.icon_size as f32)
                    .width(Length::Fixed(appearance.icon_size as f32))
                    .align_x(iced::alignment::Horizontal::Center),
            )
            .style(move |_, _status| iced::widget::button::Style {
                background: None,
                border: iced::Border::default().rounded(appearance.border_radius),
                text_color,
                ..Default::default()
            })
            .padding(0.0)
            .on_press(Message::LaunchLauncher)
            .into()
        });

        let (mut children, mut trailing): (Vec<Element<'_, Message>>, Vec<Element<'_, Message>>) =
            (Vec::new(), Vec::new());

        match launcher.position {
            config::LauncherPosition::Start => children.extend(launcher_button),
            config::LauncherPosition::End => trailing.extend(launcher_button),
        }

        // --- Application icons ---
        for group in self.groups() {
            children.push(self.app_button(&group));
        }

        children.extend(trailing);

        let content: Element<'_, Message> = row(children).spacing(appearance.spacing).into();

        container(content)
            .width(Length::Fill)
            .height(Length::Fixed(appearance.dock_size as f32))
            .align_x(iced::alignment::Horizontal::Center)
            .align_y(iced::alignment::Vertical::Center)
            .style(move |theme: &iced::Theme| container::Style {
                background: Some(iced::Background::Color(color_from_hex(
                    appearance.background_color,
                    appearance.opacity,
                ))),
                border: iced::Border {
                    color: color_from_hex(appearance.border_color, appearance.opacity),
                    width: 1.0,
                    radius: (appearance.border_radius).into(),
                },
                text_color: Some(theme.palette().text),
                ..Default::default()
            })
            .into()
    }

    fn app_icon_handle(&self, group: &AppGroup) -> Option<image::Handle> {
        let icon_name = group.app.as_ref()?.icon.clone()?;
        let size = use_theme(|a| a.icon_size.max(48));

        let mut cache = self.icon_cache.borrow_mut();
        cache
            .entry(icon_name.clone())
            .or_insert_with(|| AppRegistry::icon_path(Some(&icon_name), size))
            .as_ref()
            .map(image::Handle::from_path)
    }

    fn app_button(&self, group: &AppGroup) -> Element<'_, Message> {
        let appearance = use_theme(|a| a.clone());
        let border_radius = appearance.border_radius;
        let icon_size = appearance.icon_size as f32;

        let icon_handle = self.app_icon_handle(group);

        let icon_element: Element<'_, Message> = match icon_handle {
            Some(handle) => image(handle)
                .width(Length::Fixed(icon_size))
                .height(Length::Fixed(icon_size))
                .into(),
            None => text(
                group
                    .app
                    .as_ref()
                    .map(|a| a.name.chars().next().unwrap_or('?'))
                    .unwrap_or_else(|| group.class.chars().next().unwrap_or('?')),
            )
            .size(icon_size * 0.7)
            .into(),
        };

        let dot = appearance.show_running_indicator.then(|| {
            let size = appearance.running_indicator_size;
            let color = color_from_hex(appearance.running_indicator_color, 1.0);
            container(row![])
                .width(Length::Fixed(size))
                .height(Length::Fixed(size))
                .style(move |_theme: &iced::Theme| container::Style {
                    background: Some(iced::Background::Color(color)),
                    border: iced::Border {
                        radius: (size / 2.0).into(),
                        ..Default::default()
                    },
                    ..Default::default()
                })
        });

        let inner: Element<'_, Message> = match dot {
            Some(dot) => column![icon_element, dot]
                .spacing(2.0)
                .align_x(iced::Alignment::Center)
                .into(),
            None => icon_element,
        };

        button(inner)
            .style(move |_theme: &iced::Theme, _status| button::Style {
                background: None,
                border: iced::Border::default().rounded(border_radius),
                ..Default::default()
            })
            .padding(2.0)
            .on_press(Message::IconClicked(group.class.clone()))
            .into()
    }

    pub fn subscription(&self) -> Subscription<Message> {
        Subscription::batch(vec![
            config::subscription(&self.config_path).map(|m| match m {
                config::Message::ConfigChanged(c) => Message::ConfigChanged(c),
            }),
            output_events().map(Message::OutputEvent),
            compositor::subscribe().map(|m| match m {
                compositor::Message::Ready(windows) => Message::WindowsReady(windows),
                compositor::Message::Updated(windows) => Message::WindowsUpdated(windows),
            }),
        ])
    }
}

/// Chooses which window to focus inside an application group.
fn pick_focus_target<'a>(
    windows: &'a [compositor::WindowInfo],
    active: &Option<String>,
) -> Option<&'a compositor::WindowInfo> {
    let last = windows.last()?;
    match active {
        Some(active_addr) => windows
            .iter()
            .find(|w| &w.address == active_addr)
            .or(Some(last)),
        None => Some(last),
    }
}

async fn focus_window(address: &str) {
    let result = match compositor::hyprland_ipc::dispatch_mode().await {
        compositor::hyprland_ipc::DispatchMode::Lua => {
            compositor::hyprland_ipc::eval_dispatch(&format!(
                "hl.dsp.focus({{ window = \"{address}\" }})"
            ))
            .await
        }
        compositor::hyprland_ipc::DispatchMode::Legacy => {
            compositor::hyprland_ipc::legacy_dispatch(&format!("focuswindow address:{address}"))
                .await
        }
    };

    if let Err(e) = result {
        log::error!("Failed to focus window {address}: {e}");
    }
}

fn color_from_hex(hex: hex_color::HexColor, opacity: f32) -> iced::Color {
    iced::Color::from_rgba8(
        hex.r,
        hex.g,
        hex.b,
        (hex.a as f32 / 255.0) * opacity.clamp(0.0, 1.0),
    )
}

use log::{info, warn};
