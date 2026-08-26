use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use iced::{
    Element, Length, OutputEvent, Subscription, SurfaceId, Task, output_events, set_input_region,
    widget::{button, column, container, image, mouse_area, row, text},
};
use log::{info, warn};

use crate::{
    config,
    outputs::{Outputs, SurfaceKind},
    services::{
        apps::{AppRegistry, DesktopApp},
        compositor::{self, DockWindows},
    },
    state,
    theme::use_theme,
    utils::launcher,
};

pub struct App {
    config: config::Config,
    config_path: PathBuf,
    outputs: Outputs,
    registry: AppRegistry,
    windows: DockWindows,
    pins: state::Pins,
    /// icon name → resolved path cache
    icon_cache: RefCell<HashMap<String, Option<PathBuf>>>,
    /// Popup mode: whether the dock is currently shown.
    popup_visible: bool,
    /// Guards stale auto-hide timers.
    hide_generation: u64,
}

#[derive(Debug, Clone)]
pub enum Message {
    ConfigChanged(Box<config::Config>),
    OutputEvent(OutputEvent),
    WindowsReady(DockWindows),
    WindowsUpdated(DockWindows),
    PinsChanged(state::Pins),
    IconClicked(String),
    LaunchLauncher,
    DockEntered,
    DockLeft,
    AutoHideElapsed(u64),
    None,
}

/// A dock entry: one application slot (grouped by resolved desktop id or,
/// for unknown apps, by raw window class).
struct AppGroup {
    key: String,
    app: Option<DesktopApp>,
    windows: Vec<compositor::WindowInfo>,
}

impl App {
    pub fn new((config, config_path): (config::Config, PathBuf)) -> Self {
        let pins = state::Pins::load(&config_path);

        Self {
            config,
            config_path,
            outputs: Outputs::default(),
            registry: AppRegistry::load(),
            windows: DockWindows::default(),
            pins,
            icon_cache: RefCell::new(HashMap::new()),
            popup_visible: false,
            hide_generation: 0,
        }
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::ConfigChanged(config) => {
                info!("Config reloaded");
                self.config = *config;
                crate::theme::update(&self.config.appearance);

                // Geometry-affecting options require fresh surfaces.
                let recreate = self.outputs.recreate_all(&self.config);
                let regions = self.apply_input_regions();

                recreate.map(|_| Message::None).chain(regions)
            }
            Message::OutputEvent(event) => match event {
                OutputEvent::Added(info) => {
                    let create = self
                        .outputs
                        .create(&self.config, info.name.clone(), info.id);
                    let regions = self.apply_input_regions();

                    create.map(|_| Message::None).chain(regions)
                }
                OutputEvent::Removed(output_id) => self
                    .outputs
                    .remove(&self.config, output_id)
                    .map(|_| Message::None),
                OutputEvent::InfoChanged(_) => Task::none(),
            },
            Message::WindowsReady(windows) => {
                self.windows = windows;
                // First service tick after startup: enforce the initial
                // (hidden) popup input regions.
                self.apply_input_regions()
            }
            Message::WindowsUpdated(windows) => {
                self.windows = windows;
                Task::none()
            }
            Message::PinsChanged(pins) => {
                info!("Pins reloaded");
                self.pins = pins;
                Task::none()
            }
            Message::IconClicked(key) => self.on_icon_clicked(key),
            Message::LaunchLauncher => {
                let cmd = self.config.launcher.cmd.clone();
                if !cmd.is_empty() {
                    tokio::spawn(async move { launcher::execute(cmd).await });
                }
                Task::none()
            }
            Message::DockEntered => self.on_dock_entered(),
            Message::DockLeft => self.on_dock_left(),
            Message::AutoHideElapsed(generation) => {
                if self.hide_generation == generation {
                    self.popup_visible = false;
                }
                self.apply_input_regions()
            }
            Message::None => Task::none(),
        }
    }

    fn is_popup(&self) -> bool {
        self.config.mode == config::Mode::Popup
    }

    fn on_dock_entered(&mut self) -> Task<Message> {
        if !self.is_popup() {
            return Task::none();
        }
        // (Re-)entering cancels any pending auto-hide.
        self.hide_generation = self.hide_generation.wrapping_add(1);
        let was_visible = self.popup_visible;
        self.popup_visible = true;

        if was_visible {
            Task::none()
        } else {
            self.apply_input_regions()
        }
    }

    fn on_dock_left(&mut self) -> Task<Message> {
        if !self.is_popup() || !self.popup_visible {
            return Task::none();
        }
        let timeout = self.config.popup.autohide_timeout;
        if timeout == 0 {
            return Task::none();
        }
        self.hide_generation = self.hide_generation.wrapping_add(1);
        let generation = self.hide_generation;

        Task::perform(
            async move { tokio::time::sleep(Duration::from_secs(timeout)).await },
            move |_| Message::AutoHideElapsed(generation),
        )
    }

    /// Syncs pointer input regions with popup visibility: when hidden the dock
    /// surface must not intercept clicks and the strips must catch hover;
    /// when visible it's the opposite.
    fn apply_input_regions(&self) -> Task<Message> {
        if !self.is_popup() {
            return Task::none();
        }
        let dock_region = if self.popup_visible {
            None
        } else {
            Some(Vec::new())
        };
        let strip_region = if self.popup_visible {
            Some(Vec::new())
        } else {
            None
        };

        let mut tasks: Vec<Task<Message>> = Vec::new();
        for id in self.outputs.strip_ids() {
            tasks
                .push(set_input_region::<Message>(id, strip_region.clone()).map(|_| Message::None));
        }
        for id in self.outputs.dock_ids() {
            tasks.push(set_input_region::<Message>(id, dock_region.clone()).map(|_| Message::None));
        }

        Task::batch(tasks)
    }

    fn on_icon_clicked(&mut self, key: String) -> Task<Message> {
        let Some(group) = self.groups().into_iter().find(|g| g.key == key) else {
            return Task::none();
        };

        if let Some(target) = pick_focus_target(&group.windows, &self.windows.active) {
            let address = target.address.clone();
            tokio::spawn(async move { focus_window(&address).await });
            return Task::none();
        }

        // Not running: launch a new instance.
        if let Some(app) = group.app.filter(|a| !a.exec.is_empty()) {
            tokio::spawn(async move { launcher::execute(app.exec).await });
        } else {
            warn!("Nothing to launch for '{key}'");
        }

        Task::none()
    }

    fn groups(&self) -> Vec<AppGroup> {
        let mut order: Vec<AppGroup> = Vec::new();
        let mut index_of: HashMap<String, usize> = HashMap::new();
        // Window classes already absorbed into a pinned group.
        let mut absorbed: std::collections::HashSet<String> = Default::default();

        // Pinned apps first, in pins-file order.
        for id in self.pins.ids() {
            let Some(app) = self.registry.by_id(id).cloned() else {
                warn!("Pinned app not found: {id}");
                continue;
            };
            let keys: Vec<String> = app.match_keys().collect();
            let windows: Vec<compositor::WindowInfo> = self
                .windows
                .windows
                .iter()
                .filter(|w| keys.iter().any(|k| w.class.to_lowercase() == *k))
                .cloned()
                .collect();
            for w in &windows {
                absorbed.insert(w.class.to_lowercase());
            }

            index_of.insert(id.clone(), order.len());
            order.push(AppGroup {
                key: id.clone(),
                app: Some(app),
                windows,
            });
        }

        // Then running unpinned applications.
        for window in &self.windows.windows {
            if absorbed.contains(&window.class.to_lowercase()) {
                continue;
            }
            let app = self.registry.by_class(&window.class);
            let key = app
                .map(|a| a.id.clone())
                .unwrap_or_else(|| format!("raw:{}", window.class.to_lowercase()));

            if let Some(&i) = index_of.get(&key) {
                order[i].windows.push(window.clone());
                continue;
            }

            index_of.insert(key.clone(), order.len());
            order.push(AppGroup {
                key,
                app: app.cloned(),
                windows: vec![window.clone()],
            });
        }

        order
    }

    pub fn view(&self, id: SurfaceId) -> Element<'_, Message> {
        match self.outputs.kind(id) {
            Some(SurfaceKind::Strip) => return self.strip_view(),
            Some(SurfaceKind::Dock) => {}
            None => return row![].into(),
        }

        // A hidden popup dock renders nothing (its input region is cleared).
        if self.is_popup() && !self.popup_visible {
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

        let dock: Element<'_, Message> = container(content)
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
            .into();

        if self.is_popup() {
            mouse_area(dock)
                .on_enter(Message::DockEntered)
                .on_exit(Message::DockLeft)
                .into()
        } else {
            dock
        }
    }

    /// Transparent hotspot strip: hovering it reveals the dock.
    fn strip_view(&self) -> Element<'_, Message> {
        mouse_area(container(row![]).width(Length::Fill).height(Length::Fill))
            .on_enter(Message::DockEntered)
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
                    .unwrap_or_else(|| group.key.chars().next().unwrap_or('?')),
            )
            .size(icon_size * 0.7)
            .into(),
        };

        let dot = (!group.windows.is_empty() && appearance.show_running_indicator).then(|| {
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
            .on_press(Message::IconClicked(group.key.clone()))
            .into()
    }

    pub fn subscription(&self) -> Subscription<Message> {
        Subscription::batch(vec![
            config::subscription(self.config_path.clone()).map(|m| match m {
                config::Message::ConfigChanged(c) => Message::ConfigChanged(c),
            }),
            state::subscription(self.config_path.clone()).map(|m| match m {
                state::Message::PinsChanged(p) => Message::PinsChanged(p),
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
