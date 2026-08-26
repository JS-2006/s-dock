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

    /// Resolves all plausible icon files for this icon name, best first.
    pub fn icon_candidates(icon: Option<&str>, size: u32) -> Vec<PathBuf> {
        let Some(name) = icon else { return Vec::new() };

        if name.starts_with('/') {
            let p = PathBuf::from(name);
            return if p.is_file() { vec![p] } else { Vec::new() };
        }

        let requested = u16::try_from(size).unwrap_or(u16::MAX);
        let sizes = [requested, 128, 96, 64, 48, 32, 24, 512, 256];

        // Base names to try. Many app icons ship under a stripped or lowercased
        // filename — especially those absent from the icon theme (reverse-DNS
        // ids, Flatpak apps, pixmap-only icons).
        let mut bases: Vec<String> = Vec::new();
        let push_base = |bases: &mut Vec<String>, s: &str| {
            if !s.is_empty() && !bases.iter().any(|x| x.eq_ignore_ascii_case(s)) {
                bases.push(s.to_string());
            }
        };
        push_base(&mut bases, name);
        push_base(&mut bases, &name.to_lowercase());
        // Reverse-DNS / Flatpak style ids (org.vendor.App, org.vendor.app_appid):
        // try each dot-component and the leading segment of a `vendor_app` part.
        for comp in name.split('.') {
            push_base(&mut bases, comp);
            push_base(&mut bases, &comp.to_lowercase());
            if let Some(u) = comp.find('_') {
                push_base(&mut bases, &comp[..u]);
                push_base(&mut bases, &comp[..u].to_lowercase());
            }
        }

        let mut out: Vec<PathBuf> = Vec::new();
        let push_unique = |out: &mut Vec<PathBuf>, p: PathBuf| {
            if p.is_file() && !out.contains(&p) {
                out.push(p);
            }
        };

        let themes: Vec<String> = linicon_theme::get_icon_theme()
            .into_iter()
            .chain(std::iter::once("hicolor".to_string()))
            .collect();

        // 1) Theme-indexed lookup — fast and correct for themed icons.
        for base in &bases {
            for theme_name in &themes {
                for s in sizes {
                    if let Some(p) = freedesktop_icons::lookup(base)
                        .with_theme(theme_name)
                        .with_size(s)
                        .find()
                    {
                        push_unique(&mut out, p);
                    }
                }
            }
            for s in sizes {
                if let Some(p) = freedesktop_icons::lookup(base).with_size(s).find() {
                    push_unique(&mut out, p);
                }
            }
        }

        // A themed icon was already resolved; no need to walk the filesystem.
        if !out.is_empty() {
            return out;
        }

        // 2) Direct filesystem scan — covers icons present on disk but missing
        //    from the theme index (pixmaps, or a theme's apps/ tree).
        const EXTENSIONS: [&str; 4] = ["png", "svg", "jpg", "xpm"];
        let mut roots: Vec<PathBuf> = std::env::var_os("XDG_DATA_DIRS")
            .map(|v| {
                std::env::split_paths(&v)
                    .flat_map(|d| [d.join("icons"), d.join("pixmaps")])
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        roots.push(PathBuf::from("/usr/share/icons"));
        roots.push(PathBuf::from("/usr/share/pixmaps"));
        if let Some(home) = std::env::var_os("HOME") {
            let home = PathBuf::from(home);
            roots.push(home.join(".local/share/icons"));
            roots.push(home.join(".local/share/pixmaps"));
            roots.push(home.join(".icons"));
        }

        let subdirs = ["apps", "scalable/apps"];
        for root in &roots {
            for theme in &themes {
                for s in sizes {
                    let dir = root.join(theme).join(format!("{s}x{s}")).join("apps");
                    for base in &bases {
                        for ext in EXTENSIONS {
                            let p = dir.join(format!("{base}.{ext}"));
                            if p.is_file() {
                                push_unique(&mut out, p);
                            }
                        }
                    }
                }
                for sub in subdirs {
                    let dir = root.join(theme).join(sub);
                    for base in &bases {
                        for ext in EXTENSIONS {
                            let p = dir.join(format!("{base}.{ext}"));
                            if p.is_file() {
                                push_unique(&mut out, p);
                            }
                        }
                    }
                }
            }
            for base in &bases {
                for ext in EXTENSIONS {
                    let p = root.join(format!("{base}.{ext}"));
                    if p.is_file() {
                        push_unique(&mut out, p);
                    }
                }
            }
        }

        out
    }
}
