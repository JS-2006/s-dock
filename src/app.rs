use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use iced::{
    Element, KeyboardInteractivity, Length, OutputEvent, Subscription, SurfaceId, Task,
    output_events, set_input_region,
    widget::{button, column, container, image, mouse_area, row, scrollable, text},
};
use log::{debug, info, warn};

use crate::{
    config,
    ipc::{IpcCommand, Rect},
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
    /// Open "multiple windows" popup, if any.
    window_popup: Option<WindowPopup>,
    /// Last content size pushed to Fit-mode dock surfaces.
    applied_fit_size: Option<(u32, u32)>,
    /// Guards stale delayed popup-close tasks.
    popup_close_generation: u64,
    /// icon name → resolved path cache
    icon_cache: RefCell<HashMap<String, Option<image::Handle>>>,
    /// Popup mode: whether the dock is currently shown.
    popup_visible: bool,
    /// Guards stale auto-hide timers.
    hide_generation: u64,
    /// Active drag: (surface, group key).
    drag: Option<(SurfaceId, String)>,
    /// True once the drag crossed another pin: release must not click.
    drag_moved: bool,
    /// Group key currently under the cursor.
    hovered: Option<String>,
    /// IPC: keep the dock visible regardless of auto-hide while set.
    forced_visible: bool,
    /// IPC: shared dock geometry, kept fresh for `geometry` queries.
    dock_rect: Arc<Mutex<Option<Rect>>>,
    /// Primary output logical size (for geometry approximation).
    screen_size: Option<(i32, i32)>,
    /// Icon strip is wider than the screen: launcher is pinned, icons scroll.
    icons_overflow: bool,
    /// Whether dock surfaces currently hold keyboard (OnDemand) focus.
    keyboard_grabbed: bool,
}

#[derive(Debug, Clone)]
pub enum Message {
    ConfigChanged(Box<config::Config>),
    OutputEvent(OutputEvent),
    WindowsReady(DockWindows),
    WindowsUpdated(DockWindows),
    PinsChanged(state::Pins),
    WindowClicked(String),
    CloseWindowPopup,
    PopupEntered,
    CloseWindowPopupElapsed(u64),
    ContextMenuRequested(SurfaceId, String),
    DragStart(SurfaceId, String),
    DragRelease(SurfaceId, String),
    AppHover(String),
    AppUnhover(String),
    DragOutLaunch,
    MenuTogglePin(String),
    MenuLaunch(String),
    MenuCloseGroup(String),
    LaunchLauncher,
    DockEntered,
    DockLeftOrDragOut,
    AutoHideElapsed(u64),
    /// Wheel-driven horizontal scroll of the icon strip (px delta).
    IconScrollBy(f32),
    /// Raw key press; only Arrow keys are acted upon while overflowing.
    KeyPressed(iced::keyboard::Key),
    Ipc(IpcCommand),
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PopupKind {
    WindowList,
    AppMenu,
}

struct WindowPopup {
    kind: PopupKind,
    group_key: String,
    /// AppMenu only: whether the app is currently pinned.
    pinned: bool,
    /// AppMenu only: whether launching is possible.
    can_launch: bool,
    windows: Vec<compositor::WindowInfo>,
}

/// A dock entry: one application slot (grouped by resolved desktop id or,
/// for unknown apps, by raw window class).
struct AppGroup {
    key: String,
    app: Option<DesktopApp>,
    windows: Vec<compositor::WindowInfo>,
}

impl App {
    pub fn new(
        (config, config_path): (config::Config, PathBuf),
        dock_rect: Arc<Mutex<Option<Rect>>>,
    ) -> Self {
        let pins = state::Pins::load(&config_path);

        Self {
            config,
            config_path,
            outputs: Outputs::default(),
            registry: AppRegistry::load(),
            windows: DockWindows::default(),
            window_popup: None,
            applied_fit_size: None,
            popup_close_generation: 0,
            pins,
            icon_cache: RefCell::new(HashMap::new()),
            popup_visible: std::env::var_os("S_DOCK_FORCE_VISIBLE").is_some(),
            hide_generation: 0,
            drag: None,
            drag_moved: false,
            hovered: None,
            forced_visible: false,
            dock_rect,
            screen_size: None,
            icons_overflow: false,
            keyboard_grabbed: false,
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

                recreate.map(|_| Message::None).chain(self.refresh())
            }
            Message::OutputEvent(event) => match event {
                OutputEvent::Added(info) => {
                    self.screen_size = info.logical_size;
                    self.recompute_dock_rect();
                    let create = self
                        .outputs
                        .create(&self.config, info.name.clone(), info.id);

                    create.map(|_| Message::None).chain(self.refresh())
                }
                OutputEvent::Removed(output_id) => self
                    .outputs
                    .remove(&self.config, output_id)
                    .map(|_| Message::None),
                OutputEvent::InfoChanged(info) => {
                    self.screen_size = info.logical_size;
                    self.refresh()
                }
            },
            Message::WindowsReady(windows) => {
                self.windows = windows;
                // First service tick after startup: enforce hidden regions
                // and push the initial fit size.
                self.refresh()
            }
            Message::WindowsUpdated(windows) => {
                self.windows = windows;

                // Close the popup when none of its windows remain.
                if let Some(popup) = &self.window_popup {
                    let alive = popup
                        .windows
                        .iter()
                        .any(|w| self.windows.windows.iter().any(|x| x.address == w.address));
                    if !alive {
                        return self.close_window_popup().map(|_| Message::None);
                    }
                }

                self.refresh()
            }
            Message::PinsChanged(pins) => {
                info!("Pins reloaded");
                self.pins = pins;
                Task::none()
            }
            Message::WindowClicked(address) => {
                let address2 = address.clone();
                tokio::spawn(async move { focus_window(&address2).await });
                self.close_window_popup().map(|_| Message::None)
            }
            Message::CloseWindowPopup => {
                // Small delay so crossing the gap between the popup and the
                // dock doesn't slam the menu shut.
                self.popup_close_generation += 1;
                let generation = self.popup_close_generation;
                Task::perform(
                    async move { tokio::time::sleep(Duration::from_millis(250)).await },
                    move |_| Message::CloseWindowPopupElapsed(generation),
                )
            }
            Message::PopupEntered => {
                self.popup_close_generation += 1;
                Task::none()
            }
            Message::CloseWindowPopupElapsed(generation) => {
                if self.popup_close_generation == generation && self.window_popup.is_some() {
                    return self.close_window_popup().map(|_| Message::None);
                }
                Task::none()
            }
            Message::ContextMenuRequested(surface, key) => self.open_context_menu(surface, key),
            Message::MenuTogglePin(key) => {
                if key.ends_with(".desktop") {
                    self.pins.toggle(&key);
                } else {
                    warn!("Cannot pin '{key}': no matching .desktop entry");
                }
                self.close_window_popup().map(|_| Message::None)
            }
            Message::MenuLaunch(key) => {
                if let Some(app) = self.groups().into_iter().find_map(|g| {
                    (g.key == key)
                        .then_some(g.app)
                        .flatten()
                        .filter(|a| !a.exec.is_empty())
                }) {
                    tokio::spawn(async move { launcher::execute(app.exec).await });
                }
                self.close_window_popup().map(|_| Message::None)
            }
            Message::MenuCloseGroup(key) => {
                if let Some(group) = self.groups().into_iter().find(|g| g.key == key) {
                    for w in group.windows {
                        let address = w.address;
                        tokio::spawn(async move { close_window(&address).await });
                    }
                }
                self.close_window_popup().map(|_| Message::None)
            }
            Message::LaunchLauncher => {
                let cmd = self.config.launcher.cmd.clone();
                if !cmd.is_empty() {
                    tokio::spawn(async move { launcher::execute(cmd).await });
                }
                Task::none()
            }
            Message::DragStart(surface, key) => {
                self.drag = Some((surface, key));
                self.drag_moved = false;
                Task::none()
            }
            Message::DragRelease(surface, key) => {
                let was_dragging = self.drag.take().is_some_and(|(_, k)| k == key);
                if !was_dragging {
                    return Task::none();
                }

                // Pure reorder (the drag crossed other pins): no click.
                if self.drag_moved {
                    debug!("drag ended after reorder — no click");
                    return Task::none();
                }

                // Released without leaving the button: it's a click.
                self.on_icon_clicked(surface, key)
            }
            Message::AppHover(key) => {
                self.hovered = Some(key.clone());

                // Live reorder while dragging a pinned app over another pin.
                if let Some((_, drag_key)) = self.drag.clone()
                    && drag_key != key
                {
                    // Crossing another icon marks this as a real drag:
                    // releasing now must not launch/focus anything.
                    self.drag_moved = true;

                    if drag_key.ends_with(".desktop") && key.ends_with(".desktop") {
                        self.pins.move_onto(&drag_key, &key);
                    }
                }
                Task::none()
            }
            Message::AppUnhover(key) => {
                if self.hovered.as_deref() == Some(key.as_str()) {
                    self.hovered = None;
                }
                Task::none()
            }
            Message::DragOutLaunch => {
                let key = self.drag.take().map(|(_, key)| key);
                if let Some(app) = key.and_then(|key| {
                    self.groups().into_iter().find_map(|g| {
                        (g.key == key)
                            .then_some(g.app)
                            .flatten()
                            .filter(|a| !a.exec.is_empty())
                    })
                }) {
                    tokio::spawn(async move { launcher::execute(app.exec).await });
                }
                Task::none()
            }
            Message::DockEntered => self.on_dock_entered(),
            Message::DockLeftOrDragOut => {
                if self.drag.is_some() {
                    // Torn off the dock: launch and end the drag.
                    return self.update(Message::DragOutLaunch);
                }

                self.hovered = None;
                self.on_dock_left()
            }
            Message::AutoHideElapsed(generation) => {
                if self.forced_visible {
                    return Task::none();
                }
                if self.hide_generation == generation {
                    self.popup_visible = false;
                }
                self.apply_input_regions()
            }
            Message::IconScrollBy(dx) => scroll_icons(dx),
            Message::KeyPressed(key) => {
                if !self.icons_overflow {
                    return Task::none();
                }
                let horizontal = self.config.position.is_horizontal();
                let slot = use_theme(|a| a.icon_size as f32 * a.hover_scale.max(1.0));
                use iced::keyboard::key::Named;
                match (key, horizontal) {
                    (iced::keyboard::Key::Named(Named::ArrowRight), true)
                    | (iced::keyboard::Key::Named(Named::ArrowDown), false) => scroll_icons(slot),
                    (iced::keyboard::Key::Named(Named::ArrowLeft), true)
                    | (iced::keyboard::Key::Named(Named::ArrowUp), false) => scroll_icons(-slot),
                    _ => Task::none(),
                }
            }
            Message::Ipc(cmd) => match cmd {
                IpcCommand::ForceVisible(v) => {
                    self.forced_visible = v;
                    if v {
                        self.popup_visible = true;
                        self.apply_input_regions();
                    }
                    Task::none()
                }
                IpcCommand::Pin(id) => {
                    if !self.pins.contains(&id) {
                        self.pins.toggle(&id);
                    }
                    Task::none()
                }
                IpcCommand::Unpin(id) => {
                    if self.pins.contains(&id) {
                        self.pins.toggle(&id);
                    }
                    Task::none()
                }
                IpcCommand::Geometry => Task::none(),
            },
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

    /// Regions + Fit-size sync; call after state that affects them changes.
    fn refresh(&mut self) -> Task<Message> {
        let regions = self.apply_input_regions();
        let size_sync = self.sync_fit_size();
        regions.chain(size_sync)
    }

    /// Caps the computed size at the output's main-axis size (minus margins)
    /// and records whether the icon strip overflows the screen.
    fn cap_to_screen(&mut self, mut size: (u32, u32)) -> (u32, u32) {
        let Some((sw, sh)) = self.screen_size else {
            self.icons_overflow = false;
            return size;
        };
        let margin = self.config.appearance.margin.unsigned_abs() * 2;
        if self.config.position.is_horizontal() {
            let max_w = (sw as u32).saturating_sub(margin);
            self.icons_overflow = size.0 > max_w;
            size.0 = size.0.min(max_w);
        } else {
            let max_h = (sh as u32).saturating_sub(margin);
            self.icons_overflow = size.1 > max_h;
            size.1 = size.1.min(max_h);
        }
        size
    }

    fn sync_fit_size(&mut self) -> Task<Message> {
        if self.config.appearance.width != config::WidthMode::Fit {
            return Task::none();
        }
        let groups = self.groups().len();
        let size = use_theme(|a| crate::outputs::fit_content_size(&self.config, a, groups));
        let size = self.cap_to_screen(size);

        let mut tasks: Vec<Task<Message>> = Vec::new();

        if Some(size) != self.applied_fit_size {
            debug!("fit size -> {size:?} overflow={}", self.icons_overflow);
            self.applied_fit_size = Some(size);
            self.recompute_dock_rect();
            tasks.push(self.outputs.apply_fit_size(size).map(|_| Message::None));
        }

        // Keyboard scrolling is only meaningful while the strip overflows:
        // dock surfaces then accept OnDemand focus (click) for Arrow keys.
        if self.icons_overflow != self.keyboard_grabbed {
            self.keyboard_grabbed = self.icons_overflow;
            let ki = if self.icons_overflow {
                KeyboardInteractivity::OnDemand
            } else {
                KeyboardInteractivity::None
            };
            let ids: Vec<SurfaceId> = self.outputs.dock_ids().collect();
            tasks.extend(ids.into_iter().map(|id| {
                iced::set_keyboard_interactivity::<Message>(id, ki).map(|_| Message::None)
            }));
        }

        Task::batch(tasks)
    }

    /// Approximate the global dock rectangle from the primary output size,
    /// the dock content size, and the configured `position`. Used by the IPC
    /// `geometry` query (exact multi-output positions aren't exposed by the
    /// layer-shell backend).
    fn recompute_dock_rect(&mut self) {
        let rect = match (self.screen_size, self.applied_fit_size) {
            (Some((sw, sh)), Some((dw, dh))) => {
                let (x, y) = match self.config.position {
                    config::Position::Top => (0, 0),
                    config::Position::Bottom => (0, sh - dh as i32),
                    config::Position::Left => (0, 0),
                    config::Position::Right => (sw - dw as i32, 0),
                };
                Some(Rect { x, y, w: dw, h: dh })
            }
            _ => None,
        };
        *self.dock_rect.lock().unwrap() = rect;
    }

    fn on_icon_clicked(&mut self, surface: SurfaceId, key: String) -> Task<Message> {
        let Some(group) = self.groups().into_iter().find(|g| g.key == key) else {
            return Task::none();
        };

        // Several windows: toggle the window-list popup instead of focusing.
        if group.windows.len() > 1 {
            let already_open = self
                .window_popup
                .as_ref()
                .is_some_and(|p| p.group_key == key);
            let close = self.close_window_popup();

            if already_open {
                return close.map(|_| Message::None);
            }

            let output_id = self.outputs.output_for_surface(surface);
            let (id, task) = self.outputs.create_window_popup(
                &self.config,
                output_id,
                group.windows.len() as u32,
            );

            let _ = id;
            self.window_popup = Some(WindowPopup {
                kind: PopupKind::WindowList,
                group_key: key,
                pinned: false,
                can_launch: false,
                windows: group.windows.clone(),
            });

            return close.chain(task).map(|_| Message::None);
        }

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

    fn close_window_popup(&mut self) -> Task<()> {
        self.window_popup = None;
        self.outputs.close_window_popup()
    }

    fn open_context_menu(&mut self, surface: SurfaceId, key: String) -> Task<Message> {
        let Some(group) = self.groups().into_iter().find(|g| g.key == key) else {
            return Task::none();
        };

        let pinned = key.ends_with(".desktop") && self.pins.contains(&key);
        let can_launch = group.app.as_ref().is_some_and(|a| !a.exec.is_empty());
        let entries = 3 + usize::from(!group.windows.is_empty()) + group.windows.len();

        let output_id = self.outputs.output_for_surface(surface);
        // Replace an already-open popup (list or menu).
        let close = self.close_window_popup();
        let (id, task) = self
            .outputs
            .create_window_popup(&self.config, output_id, entries as u32);
        let _ = id;

        self.window_popup = Some(WindowPopup {
            kind: PopupKind::AppMenu,
            group_key: key,
            pinned,
            can_launch,
            windows: group.windows,
        });

        close.chain(task).map(|_| Message::None)
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
            Some(SurfaceKind::WindowList) => return self.window_list_view(id),
            Some(SurfaceKind::Dock) => {}
            None => return row![].into(),
        }

        // A hidden popup dock renders nothing (its input region is cleared).
        if self.is_popup() && !self.popup_visible {
            return row![].into();
        }

        let appearance = use_theme(|a| a.clone());
        let launcher = self.config.launcher.clone();

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

        let icons_row: Element<'_, Message> = {
            let mut kids: Vec<Element<'_, Message>> = Vec::new();
            for group in self.groups() {
                kids.push(self.app_button(&group, id));
            }
            row(kids).spacing(appearance.spacing).into()
        };

        // When the strip overflows the screen the launcher stays pinned
        // outside the scrollable area and only the icons scroll.
        let icons: Element<'_, Message> = if self.icons_overflow {
            let scroll = scrollable(icons_row)
                .direction(scrollable::Direction::Horizontal(
                    scrollable::Scrollbar::hidden(),
                ))
                .id(iced::widget::Id::new(DOCK_SCROLL_ID))
                .width(Length::Fill);
            container(mouse_area(scroll).on_scroll(|delta| match delta {
                // Vertical wheel: the horizontal scrollable ignores it natively.
                iced::mouse::ScrollDelta::Lines { x: 0.0, y } => Message::IconScrollBy(-y * 60.0),
                iced::mouse::ScrollDelta::Pixels { x: 0.0, y } => Message::IconScrollBy(-y),
                // Horizontal deltas are already handled by the scrollable.
                _ => Message::None,
            }))
            .width(Length::Fill)
            .into()
        } else {
            icons_row
        };

        let mut kids: Vec<Element<'_, Message>> = Vec::new();
        match launcher.position {
            config::LauncherPosition::Start => {
                kids.extend(launcher_button);
                kids.push(icons);
            }
            config::LauncherPosition::End => {
                kids.push(icons);
                kids.extend(launcher_button);
            }
        }

        let content: Element<'_, Message> = row(kids).spacing(appearance.spacing).into();

        // `Fit`: widen the dock past the first/last icon by the corner radius
        // so icons never sit inside the rounded-off corners.
        let fit_padding = match appearance.width {
            config::WidthMode::Fit => appearance.border_radius,
            config::WidthMode::Full => 0.0,
        };

        // Align the icon row to the screen edge so overflow (hover growth)
        // expands AWAY from the screen and dots sit on the dock's edge.
        let (align_x, align_y) = match self.config.position {
            config::Position::Top => (
                iced::alignment::Horizontal::Center,
                iced::alignment::Vertical::Top,
            ),
            config::Position::Bottom => (
                iced::alignment::Horizontal::Center,
                iced::alignment::Vertical::Bottom,
            ),
            config::Position::Left => (
                iced::alignment::Horizontal::Left,
                iced::alignment::Vertical::Center,
            ),
            config::Position::Right => (
                iced::alignment::Horizontal::Right,
                iced::alignment::Vertical::Center,
            ),
        };

        let dock: Element<'_, Message> = container(content)
            .width(Length::Fill)
            .height(Length::Fixed(appearance.dock_size as f32))
            .padding([0.0, fit_padding])
            .align_x(align_x)
            .align_y(align_y)
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

        // Horizontal docks have hover headroom in the surface: pin the visual
        // dock to its screen edge so icons overflow toward open space.
        let dock: Element<'_, Message> = if self.config.position.is_horizontal() {
            let align_y = match self.config.position {
                config::Position::Top => iced::alignment::Vertical::Top,
                _ => iced::alignment::Vertical::Bottom,
            };
            container(dock)
                .width(Length::Fill)
                .height(Length::Fill)
                .align_y(align_y)
                .into()
        } else {
            dock
        };

        if self.is_popup() {
            mouse_area(dock)
                .on_enter(Message::DockEntered)
                .on_exit(Message::DockLeftOrDragOut)
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
        let size = use_theme(|a| a.icon_size.max(48));

        // Candidate names: the .desktop icon, then the app name and the window
        // class — covers apps whose .desktop has no Icon= field or whose icon
        // isn't in the theme.
        let mut names: Vec<String> = Vec::new();
        if let Some(app) = &group.app {
            if let Some(icon) = &app.icon {
                names.push(icon.clone());
            }
            names.push(app.name.clone());
        }
        if let Some(w) = group.windows.first().filter(|w| !w.class.is_empty()) {
            names.push(w.class.clone());
        }

        for name in &names {
            if name.is_empty() {
                continue;
            }
            {
                let cache = self.icon_cache.borrow_mut();
                if let Some(handle) = cache.get(name) {
                    return handle.clone();
                }
            }

            for path in AppRegistry::icon_candidates(Some(name), size) {
                if let Some(handle) = load_icon_handle(&path, size) {
                    debug!("icon '{name}' -> {}", path.display());
                    self.icon_cache
                        .borrow_mut()
                        .insert(name.clone(), Some(handle.clone()));
                    return Some(handle);
                }
            }

            debug!("no usable icon found for '{name}'");
            self.icon_cache.borrow_mut().insert(name.clone(), None);
        }

        None
    }

    fn running_dots(
        &self,
        count: usize,
        appearance: &config::AppearanceConfig,
    ) -> Option<Element<'_, Message>> {
        (appearance.show_running_indicator && count > 0).then(|| {
            let size = appearance.running_indicator_size;
            let color = color_from_hex(appearance.running_indicator_color, 1.0);
            let n = count.clamp(1, 10);

            let dots: Vec<Element<'_, Message>> = (0..n)
                .map(|_| {
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
                        .into()
                })
                .collect();

            row(dots).spacing(3.0).into()
        })
    }

    fn app_button(&self, group: &AppGroup, surface: SurfaceId) -> Element<'_, Message> {
        let appearance = use_theme(|a| a.clone());
        let border_radius = appearance.border_radius;
        let icon_size = appearance.icon_size as f32;
        let key = group.key.clone();
        let hovered = self.hovered.as_deref() == Some(key.as_str());
        let scale = if hovered {
            appearance.hover_scale.max(1.0)
        } else {
            1.0
        };
        let scaled = icon_size * scale;

        let icon_handle = self.app_icon_handle(group);

        let icon_element: Element<'_, Message> = match icon_handle {
            Some(handle) => image(handle)
                .width(Length::Fixed(scaled))
                .height(Length::Fixed(scaled))
                .into(),
            None => {
                // No icon resolvable (e.g. a Wine/RDP app whose .desktop points at
                // a missing file): show a deterministic coloured monogram tile
                // so the dock stays visually consistent.
                let label = group
                    .app
                    .as_ref()
                    .and_then(|a| a.name.chars().next())
                    .or_else(|| key.chars().next())
                    .unwrap_or('?')
                    .to_uppercase()
                    .next()
                    .unwrap_or('?');
                let palette = [
                    iced::Color::from_rgb(0.90, 0.10, 0.29),
                    iced::Color::from_rgb(0.24, 0.71, 0.29),
                    iced::Color::from_rgb(0.26, 0.39, 0.85),
                    iced::Color::from_rgb(0.96, 0.51, 0.19),
                    iced::Color::from_rgb(0.57, 0.12, 0.71),
                    iced::Color::from_rgb(0.27, 0.94, 0.94),
                    iced::Color::from_rgb(0.94, 0.20, 0.90),
                    iced::Color::from_rgb(0.74, 0.96, 0.05),
                ];
                let hash = key.bytes().fold(0u32, |a, b| a.wrapping_add(b as u32));
                let color = palette[(hash % palette.len() as u32) as usize];
                container(
                    text(label)
                        .size(scaled * 0.5)
                        .align_x(iced::alignment::Horizontal::Center)
                        .align_y(iced::alignment::Vertical::Center),
                )
                .width(Length::Fixed(scaled))
                .height(Length::Fixed(scaled))
                .align_x(iced::Alignment::Center)
                .align_y(iced::Alignment::Center)
                .style(move |_t| container::Style {
                    background: Some(iced::Background::Color(color)),
                    border: iced::Border::default().rounded(border_radius),
                    ..Default::default()
                })
                .into()
            }
        };

        // Reserve the max footprint so neighbors never shift on hover.
        let box_w = icon_size * appearance.hover_scale.max(1.0);
        let show_dots = appearance.show_running_indicator;
        let dot_h = appearance.running_indicator_size;
        let dot_gap = if show_dots { 3.0 } else { 0.0 };
        let dots_h = if show_dots { dot_h + dot_gap } else { 0.0 };
        let box_h = icon_size * appearance.hover_scale.max(1.0) + dots_h;

        // Dots sit BELOW the icon at the panel's bottom edge. A fixed-height
        // bottom zone (empty when the app isn't running) keeps every icon's
        // baseline level, so icons without dots line up with those that have
        // them instead of dropping to the bottom of the box.
        let bottom_zone: Element<'_, Message> =
            match self.running_dots(group.windows.len(), &appearance) {
                Some(dots) => dots,
                None => container(row![]).height(Length::Fixed(dot_h)).into(),
            };

        let col: Element<'_, Message> = column![icon_element, bottom_zone]
            .spacing(dot_gap)
            .align_x(iced::Alignment::Center)
            .into();

        // NOTE: intentionally NOT a `button` widget — an iced Button captures
        // left-presses even without handlers, starving the wrapping
        // MouseArea. This container is purely visual.
        let body = container(col)
            .width(Length::Fixed(box_w))
            .height(Length::Fixed(box_h))
            .align_x(iced::Alignment::Center)
            .align_y(iced::Alignment::End)
            .style(move |_theme: &iced::Theme| container::Style {
                background: hovered
                    .then(|| iced::Background::Color(iced::Color::from_rgba8(255, 255, 255, 0.10))),
                border: iced::Border::default().rounded(border_radius),
                ..Default::default()
            });

        mouse_area(body)
            .on_press(Message::DragStart(surface, key.clone()))
            .on_release(Message::DragRelease(surface, key.clone()))
            .on_enter(Message::AppHover(key.clone()))
            .on_exit(Message::AppUnhover(key.clone()))
            .on_right_press(Message::ContextMenuRequested(surface, key))
            .into()
    }

    /// Popup content: either the plain window list or the app context menu.
    fn window_list_view(&self, id: SurfaceId) -> Element<'_, Message> {
        let Some(popup) = self.window_popup.as_ref() else {
            return row![].into();
        };
        let _ = id;

        let appearance = use_theme(|a| a.clone());
        let radius = appearance.border_radius;
        let bg = color_from_hex(appearance.background_color, appearance.opacity);
        let border_col = color_from_hex(appearance.border_color, 1.0);

        let mut rows: Vec<Element<'_, Message>> = Vec::new();

        match popup.kind {
            PopupKind::WindowList => {
                rows.extend(popup.windows.iter().map(|w| self.window_row(w)));
            }
            PopupKind::AppMenu => {
                let key = popup.group_key.clone();

                if key.ends_with(".desktop") {
                    let pin_label = if popup.pinned {
                        "Открепить"
                    } else {
                        "Закрепить"
                    };
                    rows.push(self.menu_row(pin_label, Message::MenuTogglePin(key.clone())));
                }
                if popup.can_launch {
                    rows.push(self.menu_row("Открыть", Message::MenuLaunch(key.clone())));
                }
                if !popup.windows.is_empty() {
                    rows.push(self.menu_row("Закрыть", Message::MenuCloseGroup(key)));
                    rows.push(self.divider());
                }
                rows.extend(popup.windows.iter().map(|w| self.window_row(w)));
            }
        }

        let content =
            container(column(rows).spacing(4.0))
                .padding(8.0)
                .style(move |_theme: &iced::Theme| container::Style {
                    background: Some(iced::Background::Color(bg)),
                    border: iced::Border {
                        color: border_col,
                        width: 1.0,
                        radius: radius.into(),
                    },
                    ..Default::default()
                });

        mouse_area(content.width(Length::Fill).height(Length::Shrink))
            .on_enter(Message::PopupEntered)
            .on_exit(Message::CloseWindowPopup)
            .into()
    }

    fn menu_row(&self, label: &str, msg: Message) -> Element<'_, Message> {
        let radius = use_theme(|t| t.border_radius);
        let hover_base = color_from_hex(use_theme(|t| t.background_color), 1.0);
        button(text(label.to_string()).size(14.0))
            .style(move |_: &iced::Theme, status| {
                let bg = (status == iced::widget::button::Status::Hovered)
                    .then(|| iced::Background::Color(bg_hover(hover_base)));
                button::Style {
                    background: bg,
                    border: iced::Border::default().rounded(radius),
                    ..Default::default()
                }
            })
            .width(Length::Fill)
            .padding([6.0, 8.0])
            .on_press(msg)
            .into()
    }

    fn divider(&self) -> Element<'_, Message> {
        container(row![])
            .height(Length::Fixed(1.0))
            .width(Length::Fill)
            .style(move |theme: &iced::Theme| container::Style {
                background: Some(iced::Background::Color(theme.palette().text)),
                ..Default::default()
            })
            .into()
    }

    fn window_row(&self, w: &compositor::WindowInfo) -> Element<'_, Message> {
        let radius = use_theme(|t| t.border_radius);
        let hover_base = color_from_hex(use_theme(|t| t.background_color), 1.0);
        let title = truncate(&w.title, 48);
        button(
            row![text("\u{25aa}").size(10.0), text(title).size(14.0)]
                .spacing(8.0)
                .align_y(iced::Alignment::Center),
        )
        .style(move |_: &iced::Theme, status| {
            let bg = (status == iced::widget::button::Status::Hovered)
                .then(|| iced::Background::Color(bg_hover(hover_base)));
            button::Style {
                background: bg,
                border: iced::Border::default().rounded(radius),
                ..Default::default()
            }
        })
        .width(Length::Fill)
        .padding([6.0, 8.0])
        .on_press(Message::WindowClicked(w.address.clone()))
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
            crate::ipc::subscription().map(Message::Ipc),
            compositor::subscribe().map(|m| match m {
                compositor::Message::Ready(windows) => Message::WindowsReady(windows),
                compositor::Message::Updated(windows) => Message::WindowsUpdated(windows),
            }),
            iced::keyboard::listen().map(|event| match event {
                iced::keyboard::Event::KeyPressed { key, .. } => Message::KeyPressed(key),
                _ => Message::None,
            }),
        ])
    }
}

const DOCK_SCROLL_ID: &str = "s-dock-icons";

/// Scrolls the icon strip by `dx` px; the scrollable clamps at both ends.
fn scroll_icons(dx: f32) -> Task<Message> {
    let op = iced::core::widget::operation::scrollable::scroll_by(
        iced::widget::Id::new(DOCK_SCROLL_ID),
        iced::widget::scrollable::AbsoluteOffset { x: dx, y: 0.0 },
    );
    Task::from(iced_runtime::task::widget::<()>(op).map(|_| Message::None))
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
            // Lua window selectors use window-rules syntax: "address:0x...".
            compositor::hyprland_ipc::eval_dispatch(&format!(
                "hl.dsp.focus({{ window = \"address:{address}\" }})"
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

/// Closes a single window by address.
///
/// Lua mode requires the TABLE form `{ window = ... }`: a bare string
/// argument is silently discarded by the binding and the dispatcher then
/// falls back to closing the FOCUSED window.
async fn close_window(address: &str) {
    let result = match compositor::hyprland_ipc::dispatch_mode().await {
        compositor::hyprland_ipc::DispatchMode::Lua => {
            compositor::hyprland_ipc::eval_dispatch(&format!(
                "hl.dsp.window.close({{ window = \"address:{address}\" }})"
            ))
            .await
        }
        compositor::hyprland_ipc::DispatchMode::Legacy => {
            compositor::hyprland_ipc::legacy_dispatch(&format!("closewindow address:{address}"))
                .await
        }
    };

    if let Err(e) = result {
        log::error!("Failed to close window {address}: {e}");
    }
}

fn bg_hover(base: iced::Color) -> iced::Color {
    iced::Color {
        a: base.a * 0.35,
        ..base
    }
}

fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        format!(
            "{}\u{2026}",
            s.chars()
                .take(max_chars.saturating_sub(1))
                .collect::<String>()
        )
    }
}

/// Decodes an icon file into a premade RGBA handle. SVGs are rasterized via
/// resvg — they must not reach the raster decoder.
fn load_icon_handle(path: &PathBuf, size_px: u32) -> Option<image::Handle> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();

    if ext == "svg" {
        let bytes = std::fs::read(path).ok()?;
        let opt = resvg::usvg::Options::default();
        let tree = resvg::usvg::Tree::from_data(&bytes, &opt).ok()?;

        let svg_size = tree.size();
        let scale = (size_px as f32 / svg_size.width()).max(0.1);
        let (w, h) = (
            (svg_size.width() * scale).round().max(1.0) as u32,
            (svg_size.height() * scale).round().max(1.0) as u32,
        );

        let mut pixmap = resvg::tiny_skia::Pixmap::new(w, h)?;
        resvg::render(
            &tree,
            resvg::tiny_skia::Transform::from_scale(scale, scale),
            &mut pixmap.as_mut(),
        );
        return Some(image::Handle::from_rgba(w, h, pixmap.take()));
    }

    let bytes = std::fs::read(path).ok()?;
    let img = ::image::load_from_memory(&bytes).ok()?;
    let (w, h) = (img.width(), img.height());
    if w == 0 || h == 0 {
        return None;
    }
    Some(image::Handle::from_rgba(w, h, img.into_rgba8().into_raw()))
}

fn color_from_hex(hex: hex_color::HexColor, opacity: f32) -> iced::Color {
    iced::Color::from_rgba8(
        hex.r,
        hex.g,
        hex.b,
        (hex.a as f32 / 255.0) * opacity.clamp(0.0, 1.0),
    )
}
