//! Global theme state derived from the `[appearance]` config section.

use std::sync::RwLock;

use crate::config::AppearanceConfig;

static THEME: RwLock<Option<AppearanceConfig>> = RwLock::new(None);

pub fn init(appearance: &AppearanceConfig) {
    if let Ok(mut theme) = THEME.write() {
        *theme = Some(appearance.clone());
    }
}

pub fn update(appearance: &AppearanceConfig) {
    init(appearance);
}

/// Runs `f` with the current appearance settings.
pub fn use_theme<T>(f: impl FnOnce(&AppearanceConfig) -> T) -> T {
    let guard = THEME.read().expect("theme lock poisoned");
    let default = AppearanceConfig::default();
    f(guard.as_ref().unwrap_or(&default))
}
