//! Management of the dock layer surfaces across outputs.
//!
//! Mirrors the ashell-ru strategy: the initial surface created by
//! `.layer_shell()` acts as a fallback until real outputs are detected; when
//! the last output disappears the fallback is recreated.

use iced::{
    Anchor, KeyboardInteractivity, Layer, LayerShellSettings, OutputId, SurfaceId, Task,
    destroy_layer_surface, new_layer_surface,
};
use log::info;

use crate::{
    config::{Alignment, Config, Mode, Position, WidthMode},
    theme::use_theme,
};

struct DockSurface {
    id: SurfaceId,
    output_id: OutputId,
    name: String,
}

#[derive(Default)]
pub struct Outputs {
    surfaces: Vec<DockSurface>,
    /// Current fallback surface; `SurfaceId::MAIN` until replaced.
    fallback: Option<SurfaceId>,
}

impl Outputs {
    pub fn is_dock(&self, id: SurfaceId) -> bool {
        self.fallback == Some(id) || self.surfaces.iter().any(|s| s.id == id)
    }

    fn destroy_fallback(&mut self) -> Task<()> {
        match self.fallback.take() {
            Some(id) => destroy_layer_surface(id),
            None => Task::none(),
        }
    }

    /// Creates a dock surface bound to the given output, replacing the
    /// fallback surface on the first call.
    pub fn create(&mut self, config: &Config, name: String, output_id: OutputId) -> Task<()> {
        let (id, task) = new_layer_surface(dock_layer_settings(config, Some(output_id)));
        info!("Created dock surface {id:?} for output {name}");

        let replace_fallback = self.fallback.is_some();
        self.surfaces.push(DockSurface {
            id,
            output_id,
            name,
        });

        if replace_fallback {
            self.destroy_fallback()
        } else {
            Task::none()
        }
        .chain(task)
    }

    /// Destroys every per-output surface, keeping the fallback alive, and
    /// recreates them — used when geometry-affecting config changes.
    pub fn recreate_all(&mut self, config: &Config) -> Task<()> {
        let existing: Vec<(String, OutputId)> = self
            .surfaces
            .iter()
            .map(|s| (s.name.clone(), s.output_id))
            .collect();

        let mut tasks: Vec<Task<()>> = self
            .surfaces
            .drain(..)
            .map(|s| destroy_layer_surface(s.id))
            .collect();

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
        let destroy_task = destroy_layer_surface(surface.id);

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

        LayerShellSettings {
            namespace: "s-dock".into(),
            anchor: dock_anchor(config.position, config.alignment, stretch),
            layer: Layer::Top,
            exclusive_zone: match config.mode {
                Mode::Fixed => thickness as i32,
                Mode::Popup => 0,
            },
            size: Some((w, h)),
            keyboard_interactivity: KeyboardInteractivity::None,
            output,
            ..Default::default()
        }
    })
}
