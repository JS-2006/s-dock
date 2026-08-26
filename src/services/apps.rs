//! Desktop application registry: `.desktop` entries, class matching and
//! icon lookup.

use std::collections::HashMap;
use std::path::PathBuf;

use freedesktop_desktop_entry::{DesktopEntry, Iter, default_paths};

/// A launchable application parsed from a `.desktop` file.
#[derive(Debug, Clone)]
pub struct DesktopApp {
    /// Desktop file id, e.g. `firefox.desktop`.
    pub id: String,
    pub name: String,
    /// Cleaned `Exec` value (field codes stripped).
    pub exec: String,
    pub icon: Option<String>,
}

impl DesktopApp {
    pub fn load_all() -> Vec<Self> {
        let locale = std::env::var("LANG").ok();

        Iter::new(default_paths())
            .entries(None::<&[&str]>)
            .filter(|e| !e.no_display())
            .map(|entry| Self::from_entry(&entry, locale.as_deref()))
            .collect()
    }

    fn from_entry(entry: &DesktopEntry, locale: Option<&str>) -> Self {
        let locales: Vec<&str> = locale
            .and_then(|l| l.split('.').next())
            .map(|l| vec![l, "en", "C"])
            .unwrap_or_else(|| vec!["en", "C"]);

        let name = entry
            .name(&locales)
            .unwrap_or_else(|| entry.appid.clone().into())
            .to_string();

        Self {
            id: format!("{}.desktop", entry.appid),
            name,
            exec: entry.exec().map(clean_exec).unwrap_or_default(),
            icon: entry.icon().map(str::to_string),
        }
    }

    /// Lookup keys used to match a window `class` against this app.
    pub fn match_keys(&self) -> impl Iterator<Item = String> + '_ {
        let stem = self.id.strip_suffix(".desktop").unwrap_or(&self.id);

        [
            Some(stem.to_lowercase()),
            self.icon.as_ref().map(|i| i.to_lowercase()),
            Some(self.name.to_lowercase()),
        ]
        .into_iter()
        .flatten()
        .filter(|k| !k.is_empty())
    }
}

/// Strips field codes (`%f`, `%U`, …) and surrounding quotes handling from an
/// `Exec` value.
fn clean_exec(exec: &str) -> String {
    exec.split_whitespace()
        .filter(|token| !token.starts_with('%'))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Registry mapping window classes to desktop applications.
#[derive(Debug, Default, Clone)]
pub struct AppRegistry {
    apps: Vec<DesktopApp>,
    /// lowercased match key → app index.
    index: HashMap<String, usize>,
}

impl AppRegistry {
    pub fn load() -> Self {
        let mut registry = Self::default();
        registry.reload();
        registry
    }

    pub fn reload(&mut self) {
        let mut apps = DesktopApp::load_all();
        apps.sort_by(|a, b| a.name.cmp(&b.name));
        apps.dedup_by(|a, b| a.id == b.id);

        let mut index = HashMap::new();
        for (i, app) in apps.iter().enumerate() {
            for key in app.match_keys() {
                index.entry(key).or_insert(i);
            }
        }

        self.apps = apps;
        self.index = index;
    }

    /// Finds an app by its desktop file id (case-insensitive).
    pub fn by_id(&self, id: &str) -> Option<&DesktopApp> {
        self.apps.iter().find(|a| a.id.eq_ignore_ascii_case(id))
    }

    /// Finds the app matching a window class (case-insensitive).
    pub fn by_class(&self, class: &str) -> Option<&DesktopApp> {
        let key = class.to_lowercase();
        self.index.get(&key).map(|&i| &self.apps[i])
    }

    /// Resolves an icon to a file path on disk.
    ///
    /// Resolution order: absolute path from `Icon=`, themed lookup over a
    /// ladder of common sizes, unthemed lookup, legacy `pixmaps` dirs.
    pub fn icon_path(icon: Option<&str>, size: u32) -> Option<PathBuf> {
        let name = icon?;
        let requested = u16::try_from(size).unwrap_or(u16::MAX);

        // Some .desktop files point straight at a file.
        if name.starts_with('/') {
            let p = PathBuf::from(name);
            return p.is_file().then_some(p);
        }

        let sizes = [requested, 128, 96, 64, 48, 64 * 2, 32, 24, 512, 256];

        let theme = linicon_theme::get_icon_theme();

        for theme_name in theme.iter() {
            for s in sizes {
                if let Some(path) = freedesktop_icons::lookup(name)
                    .with_theme(theme_name)
                    .with_size(s)
                    .find()
                {
                    return Some(path);
                }
            }
        }

        // Explicit hicolor pass (fallback theme every spec-compliant setup has).
        for s in sizes {
            if let Some(path) = freedesktop_icons::lookup(name)
                .with_theme("hicolor")
                .with_size(s)
                .find()
            {
                return Some(path);
            }
        }

        for s in sizes {
            if let Some(path) = freedesktop_icons::lookup(name).with_size(s).find() {
                return Some(path);
            }
        }

        // Any size at all (e.g. scalable-only icons outside the theme).
        if let Some(path) = freedesktop_icons::lookup(name).find() {
            return Some(path);
        }

        pixmap_fallback(name)
    }
}

/// Legacy lookup in `<data-dir>/pixmaps` for apps whose icons never made it
/// into an icon theme.
fn pixmap_fallback(name: &str) -> Option<PathBuf> {
    const EXTENSIONS: [&str; 2] = ["png", "svg"];

    let mut dirs: Vec<PathBuf> = std::env::var_os("XDG_DATA_DIRS")
        .map(|v| {
            std::env::split_paths(&v)
                .map(|d| d.join("pixmaps"))
                .collect()
        })
        .unwrap_or_default();
    dirs.push(PathBuf::from("/usr/share/pixmaps"));
    dirs.push(
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default()
            .join(".local/share/pixmaps"),
    );

    dirs.into_iter()
        .flat_map(|dir| {
            EXTENSIONS
                .iter()
                .map(move |ext| dir.join(format!("{name}.{ext}")))
        })
        .find(|p| p.is_file())
}
