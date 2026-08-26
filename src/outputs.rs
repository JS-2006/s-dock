//! Management of the dock layer surfaces across outputs.
//!
//! Mirrors the ashell-ru strategy: the initial surface created by
//! `.layer_shell()` acts as a fallback until real outputs are detected; when
//! the last output disappears the fallback is recreated.
//!
//! In `Popup` mode every output also gets a thin transparent hotspot strip
//! surface at the screen edge; hovering it reveals the dock.

use iced::{
    Anchor, KeyboardInteractivity, Layer, LayerShellSettings, OutputId, SurfaceId, Task,
    destroy_layer_surface, new_layer_surface,
};
use log::info;

use crate::{
    config::{Alignment, Config, HotspotWidth, Mode, Position, WidthMode},
    theme::use_theme,
};

struct OutputDock {
    dock_id: SurfaceId,
    strip_id: Option<SurfaceId>,
    output_id: OutputId,
    name: String,
}

#[derive(Default)]
pub struct Outputs {
    surfaces: Vec<OutputDock>,
    /// Current fallback surface; `SurfaceId::MAIN` until replaced.
    fallback: Option<SurfaceId>,
    /// At most one window-list popup at a time.
    window_popup: Option<(SurfaceId, Option<OutputId>)>,
}

/// Which surface a given `SurfaceId` refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceKind {
    Dock,
    Strip,
    WindowList,
}

impl Outputs {
    pub fn kind(&self, id: SurfaceId) -> Option<SurfaceKind> {
        if self.fallback == Some(id) {
            return Some(SurfaceKind::Dock);
        }
        if self.window_popup.iter().any(|(pid, _)| *pid == id) {
            return Some(SurfaceKind::WindowList);
        }
        for s in &self.surfaces {
            if s.dock_id == id {
                return Some(SurfaceKind::Dock);
            }
            if s.strip_id == Some(id) {
                return Some(SurfaceKind::Strip);
            }
        }
        None
    }

    /// Output bound to a dock/strip surface (fallback returns `None`).
    pub fn output_for_surface(&self, id: SurfaceId) -> Option<OutputId> {
        self.surfaces
            .iter()
            .find(|s| s.dock_id == id || s.strip_id == Some(id))
            .map(|s| s.output_id)
    }

    pub fn create_window_popup(
        &mut self,
        config: &Config,
        output_id: Option<OutputId>,
        entries: u32,
    ) -> (SurfaceId, Task<()>) {
        let (id, task) = new_layer_surface(window_list_settings(config, output_id, entries));
        info!("Created window-list popup {id:?}");

        let previous = self.window_popup.replace((id, output_id));
        let close_old = previous
            .map(|(old, _)| destroy_layer_surface(old))
            .unwrap_or_else(Task::none);

        let chained = close_old.chain(task);
        (id, chained)
    }

    pub fn close_window_popup(&mut self) -> Task<()> {
        match self.window_popup.take() {
            Some((id, _)) => destroy_layer_surface(id),
            None => Task::none(),
        }
    }

    pub fn dock_ids(&self) -> impl Iterator<Item = SurfaceId> + '_ {
        self.fallback
            .into_iter()
            .chain(self.surfaces.iter().map(|s| s.dock_id))
    }

    pub fn strip_ids(&self) -> impl Iterator<Item = SurfaceId> + '_ {
        self.surfaces.iter().filter_map(|s| s.strip_id)
    }

    fn destroy_fallback(&mut self) -> Task<()> {
        match self.fallback.take() {
            Some(id) => destroy_layer_surface(id),
            None => Task::none(),
        }
    }

    /// Creates the dock surface (and the hotspot strip in popup mode) bound to
    /// the given output, replacing the fallback surface on the first call.
    pub fn create(&mut self, config: &Config, name: String, output_id: OutputId) -> Task<()> {
        let (dock_id, task) = new_layer_surface(dock_layer_settings(config, Some(output_id)));
        info!("Created dock surface {dock_id:?} for output {name}");

        let strip_task = if config.mode == Mode::Popup {
            let (strip_id, task) = new_layer_surface(strip_layer_settings(config, Some(output_id)));
            info!("Created hotspot strip {strip_id:?} for output {name}");
            self.surfaces.push(OutputDock {
                dock_id,
                strip_id: Some(strip_id),
                output_id,
                name,
            });
            task
        } else {
            self.surfaces.push(OutputDock {
                dock_id,
                strip_id: None,
                output_id,
                name,
            });
            Task::none()
        };

        let replace_fallback = self.fallback.is_some();

        if replace_fallback {
            self.destroy_fallback()
        } else {
            Task::none()
        }
        .chain(task)
        .chain(strip_task)
    }

    /// Destroys every per-output surface, keeping the fallback alive, and
    /// recreates them — used when geometry-affecting config changes.
    pub fn recreate_all(&mut self, config: &Config) -> Task<()> {
        let existing: Vec<(String, OutputId)> = self
            .surfaces
            .iter()
            .map(|s| (s.name.clone(), s.output_id))
            .collect();

        let mut tasks: Vec<Task<()>> = Vec::new();
        if let Some((popup, _)) = self.window_popup.take() {
            tasks.push(destroy_layer_surface(popup));
        }
        for s in self.surfaces.drain(..) {
            tasks.push(destroy_layer_surface(s.dock_id));
            if let Some(strip) = s.strip_id {
                tasks.push(destroy_layer_surface(strip));
            }
        }

        for (name, output_id) in existing {
            tasks.push(self.create(config, name, output_id));
        }

        Task::batch(tasks)
    }

    pub fn remove(&mut self, config: &Config, output_id: OutputId) -> Task<()> {
        let Some(pos) = self.surfaces.iter().position(|s| s.output_id == output_id) else {
            return Task::none();
        };

        let surface = self.surfaces.remove(pos);
        let mut tasks = vec![destroy_layer_surface(surface.dock_id)];
        if let Some(strip) = surface.strip_id {
            tasks.push(destroy_layer_surface(strip));
        }
        if let Some((popup, owner)) = self.window_popup
            && owner.is_some_and(|owner| owner == output_id)
        {
            tasks.push(destroy_layer_surface(popup));
            self.window_popup = None;
        }
        let destroy_task = Task::batch(tasks);

        // No outputs left: bring the fallback surface back.
        if self.surfaces.is_empty() && self.fallback.is_none() {
            let (id, task) = new_layer_surface(dock_layer_settings(config, None));
            info!("No outputs left, recreated fallback surface {id:?}");
            self.fallback = Some(id);

            return destroy_task.chain(task);
        }

        destroy_task
    }
}

/// Anchor for the dock surface following `position` × `alignment`.
///
/// On the dock's main axis both edges are anchored only for `Full` width so
/// the surface stretches; otherwise the surface keeps its computed size and is
/// placed at the aligned edge (no anchor on that axis = centered).
fn dock_anchor(position: Position, alignment: Alignment, stretch: bool) -> Anchor {
    let main = match position {
        Position::Top => Anchor::TOP,
        Position::Bottom => Anchor::BOTTOM,
        Position::Left => Anchor::LEFT,
        Position::Right => Anchor::RIGHT,
    };

    let cross_stretch = if position.is_horizontal() {
        Anchor::LEFT | Anchor::RIGHT
    } else {
        Anchor::TOP | Anchor::BOTTOM
    };

    let cross = match alignment {
        Alignment::Center => {
            return if stretch { main | cross_stretch } else { main };
        }
        Alignment::Start => match position {
            Position::Top | Position::Bottom => Anchor::LEFT,
            Position::Left | Position::Right => Anchor::TOP,
        },
        Alignment::End => match position {
            Position::Top | Position::Bottom => Anchor::RIGHT,
            Position::Left | Position::Right => Anchor::BOTTOM,
        },
    };

    main | cross
}

/// Layer-shell settings for the dock surface (also used for the initial
/// fallback surface created by `.layer_shell()` in main.rs).
pub fn dock_layer_settings(config: &Config, output: Option<OutputId>) -> LayerShellSettings {
    use_theme(|appearance| {
        let horizontal = config.position.is_horizontal();
        let thickness = appearance.dock_size;
        let stretch = appearance.width == WidthMode::Full;

        // Stage 1 stub: `Fit` docks get a placeholder square until real
        // content sizing lands with the app list.
        let (w, h) = match (horizontal, stretch) {
            (true, true) => (0, thickness),
            (true, false) => (thickness * 4, thickness),
            (false, true) => (thickness, 0),
            (false, false) => (thickness, thickness * 4),
        };

        // Room for the hover-scale icon to overflow past the dock edge.
        let headroom = if horizontal {
            (((appearance.hover_scale - 1.0).max(0.0)) * appearance.icon_size as f32) as u32
        } else {
            0
        };

        let m = appearance.margin;
        let r = appearance.border_radius as i32;
        // With `Full` width the rounded corners would leave gaps at the screen
        // edges: extend the surface past the edges by the corner radius.
        let overhang = (stretch && appearance.width == WidthMode::Full).then_some(r);
        let layer = match config.mode {
            Mode::Fixed => Layer::Top,
            Mode::Popup => Layer::Overlay,
        };
        let exclusive_zone = match config.mode {
            Mode::Fixed => thickness as i32,
            // -1 = ignore other surfaces' exclusive zones too.
            Mode::Popup => -1,
        };
        let margin = match (horizontal, overhang) {
            (true, Some(r)) => (m, -r, m, -r),
            (false, Some(r)) => (-r, m, -r, m),
            _ => (m, m, m, m),
        };

        info!(
            "dock surface: mode={:?} layer={:?} exclusive_zone={} anchor={:?} size=({w},{h}) margin=({:?})",
            config.mode, layer, exclusive_zone, config.alignment, margin
        );

        LayerShellSettings {
            namespace: "s-dock".into(),
            anchor: dock_anchor(config.position, config.alignment, stretch),
            // Popup floats above everything without shifting windows;
            // Fixed sits in its own reserved strip below windows.
            layer,
            exclusive_zone,
            size: Some((w, h + headroom)),
            margin,
            keyboard_interactivity: KeyboardInteractivity::None,
            output,
        }
    })
}

/// Layer-shell settings for the transparent hotspot strip.
///
/// `HotspotWidth::Dock` spans the whole screen edge; `Pixels(n)` is a centered
/// stripe of exactly `n` logical pixels.
pub fn strip_layer_settings(config: &Config, output: Option<OutputId>) -> LayerShellSettings {
    use_theme(|appearance| {
        let thickness = config.popup.hotspot_height;

        let anchor = match config.popup.hotspot_width {
            HotspotWidth::Dock => dock_anchor(config.position, config.alignment, true),
            HotspotWidth::Pixels(_) => dock_anchor(config.position, config.alignment, false),
        };

        let (w, h) = match (config.position.is_horizontal(), config.popup.hotspot_width) {
            (true, HotspotWidth::Dock) => (0, thickness),
            (true, HotspotWidth::Pixels(px)) => (px, thickness),
            (false, HotspotWidth::Dock) => (thickness, 0),
            (false, HotspotWidth::Pixels(px)) => (thickness, px),
        };

        let m = appearance.margin;
        let margin = (m, m, m, m);

        LayerShellSettings {
            namespace: "s-dock-strip".into(),
            margin,
            anchor,
            layer: Layer::Overlay,
            exclusive_zone: -1,
            size: Some((w, h)),
            keyboard_interactivity: KeyboardInteractivity::None,
            output,
        }
    })
}

/// Layer-shell settings for the window-list popup: floats above the dock on
/// the same output, aligned with it.
pub fn window_list_settings(
    config: &Config,
    output: Option<OutputId>,
    entries: u32,
) -> LayerShellSettings {
    use_theme(|appearance| {
        let entry_h = config.popup.entry_height.max(24);
        let pad = 8;
        let width = config.popup.menu_width.max(160);
        let visible = entries.clamp(1, config.popup.max_entries.max(1));

        let (w, h) = (width, visible * entry_h + pad * 2);

        let lift = appearance.dock_size as i32
            + appearance.margin * 2
            + config.popup.hotspot_height as i32;
        let m = appearance.margin;
        let margin = match config.position {
            Position::Bottom => (0, m, lift, m),
            Position::Top => (lift, m, 0, m),
            Position::Left => (m, lift, m, 0),
            Position::Right => (m, 0, m, lift),
        };

        LayerShellSettings {
            namespace: "s-dock-winlist".into(),
            anchor: dock_anchor(config.position, config.alignment, false),
            layer: Layer::Overlay,
            exclusive_zone: -1,
            size: Some((w, h)),
            margin,
            keyboard_interactivity: KeyboardInteractivity::None,
            output,
        }
    })
}
