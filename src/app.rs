use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use iced::{
    Element, Length, OutputEvent, Subscription, SurfaceId, Task, output_events, set_input_region,
    widget::{button, column, container, image, mouse_area, row, text},
};
use log::{debug, info, warn};

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
    pub fn new((config, config_path): (config::Config, PathBuf)) -> Self {
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
            popup_visible: false,
            hide_generation: 0,
            drag: None,
            drag_moved: false,
            hovered: None,
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
                    let create = self
                        .outputs
                        .create(&self.config, info.name.clone(), info.id);

                    create.map(|_| Message::None).chain(self.refresh())
                }
                OutputEvent::Removed(output_id) => self
                    .outputs
                    .remove(&self.config, output_id)
                    .map(|_| Message::None),
                OutputEvent::InfoChanged(_) => Task::none(),
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

    /// Regions + Fit-size sync; call after state that affects them changes.
    fn refresh(&mut self) -> Task<Message> {
        let regions = self.apply_input_regions();
        let size_sync = self.sync_fit_size();
        regions.chain(size_sync)
    }

    fn sync_fit_size(&mut self) -> Task<Message> {
        if self.config.appearance.width != config::WidthMode::Fit {
            return Task::none();
        }
        let children = self.groups().len() + usize::from(self.config.launcher.enabled);
        let size = use_theme(|a| crate::outputs::fit_content_size(&self.config, a, children));
        if Some(size) == self.applied_fit_size {
            return Task::none();
        }
        debug!("fit size -> {:?}", size);
        self.applied_fit_size = Some(size);
        self.outputs.apply_fit_size(size).map(|_| Message::None)
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
            children.push(self.app_button(&group, id));
        }

        children.extend(trailing);

        let content: Element<'_, Message> = row(children).spacing(appearance.spacing).into();

        // `Fit`: widen the dock past the first/last icon by the corner radius
        // so icons never sit inside the rounded-off corners.
        let fit_padding = match appearance.width {
            config::WidthMode::Fit => appearance.border_radius,
            config::WidthMode::Full => 0.0,
        };

        let dock: Element<'_, Message> = container(content)
            .width(Length::Fill)
            .height(Length::Fixed(appearance.dock_size as f32))
            .padding([0.0, fit_padding])
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
        let icon_name = group.app.as_ref()?.icon.clone()?;
        let size = use_theme(|a| a.icon_size.max(48));

        {
            let cache = self.icon_cache.borrow_mut();
            if let Some(handle) = cache.get(&icon_name) {
                return handle.clone();
            }
        }

        // Failures are not cached: a theme/icon install should recover.
        for path in AppRegistry::icon_candidates(Some(&icon_name), size) {
            if let Some(handle) = load_icon_handle(&path, size) {
                debug!("icon '{icon_name}' -> {}", path.display());
                self.icon_cache
                    .borrow_mut()
                    .insert(icon_name, Some(handle.clone()));
                return Some(handle);
            }
        }

        debug!("no usable icon found for '{icon_name}'");
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
            None => text(
                group
                    .app
                    .as_ref()
                    .map(|a| a.name.chars().next().unwrap_or('?'))
                    .unwrap_or_else(|| key.chars().next().unwrap_or('?')),
            )
            .size(scaled * 0.7)
            .into(),
        };

        // Reserve the max footprint so neighbors never shift on hover.
        let box_w = icon_size * appearance.hover_scale.max(1.0);
        let dots_h = if appearance.show_running_indicator {
            appearance.running_indicator_size + 3.0
        } else {
            0.0
        };
        let box_h = icon_size * appearance.hover_scale.max(1.0) + dots_h;

        // Dots sit ABOVE the icon: with the button bottom-aligned, a scaled
        // icon grows upward and pushes them along instead of clipping.
        let col: Element<'_, Message> = match self.running_dots(group.windows.len(), &appearance) {
            Some(dots) => column![dots, icon_element]
                .spacing(3.0)
                .align_x(iced::Alignment::Center)
                .into(),
            None => icon_element,
        };

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
