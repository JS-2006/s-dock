//! Desktop application registry: `.desktop` entries, class matching and
//! icon lookup.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

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
    /// `StartupWMClass` from the desktop entry, used to match running windows
    /// whose class doesn't resemble the app id (e.g. Wine apps).
    pub wm_class: Option<String>,
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
            wm_class: entry.startup_wm_class().map(str::to_string),
        }
    }

    /// Lookup keys used to match a window `class` against this app.
    pub fn match_keys(&self) -> impl Iterator<Item = String> + '_ {
        let stem = self.id.strip_suffix(".desktop").unwrap_or(&self.id);

        [
            Some(stem.to_lowercase()),
            self.icon.as_ref().map(|i| i.to_lowercase()),
            Some(self.name.to_lowercase()),
            self.wm_class.as_ref().map(|w| w.to_lowercase()),
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
    ///
    /// Strategy (robust and theme-agnostic): first ask the freedesktop icon
    /// theme resolver (respects the user's theme + inheritance), then fall back
    /// to a one-time global index of every icon file on disk. That way any icon
    /// present anywhere — pixmaps, an unindexed theme directory, a different
    /// theme — is found by any reasonable name variant, independent of the
    /// on-disk directory layout.
    pub fn icon_candidates(icon: Option<&str>, size: u32) -> Vec<PathBuf> {
        let Some(name) = icon else { return Vec::new() };

        if name.starts_with('/') {
            let p = PathBuf::from(name);
            return if p.is_file() { vec![p] } else { Vec::new() };
        }

        let requested = u16::try_from(size).unwrap_or(u16::MAX);
        let names = candidate_names(name);
        let mut out: Vec<PathBuf> = Vec::new();
        let push_unique = |out: &mut Vec<PathBuf>, p: &Path| {
            if p.is_file() && !out.iter().any(|x| x == p) {
                out.push(p.to_path_buf());
            }
        };

        // 1) Proper themed lookup — correct for icons present in a theme index.
        for n in &names {
            if let Some(p) = freedesktop_icons::lookup(n).with_size(requested).find() {
                push_unique(&mut out, &p);
            }
        }

        if !out.is_empty() {
            return out;
        }

        // 2) Global index fallback — covers everything the theme index misses.
        let idx = icon_index();
        for n in &names {
            if let Some(p) = idx.by_stem.get(&n.to_lowercase()) {
                push_unique(&mut out, p);
            }
        }

        out
    }
}

/// Name variants to try when resolving an icon: the name itself, lowercased,
/// and every segment obtained by splitting on `.`, `_` and `-` (so a Flatpak /
/// reverse-DNS id like `org.localsend.localsend_app` also yields `localsend`).
fn candidate_names(name: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let push = |out: &mut Vec<String>, s: &str| {
        if !s.is_empty() && !out.iter().any(|x| x.eq_ignore_ascii_case(s)) {
            out.push(s.to_string());
        }
    };
    push(&mut out, name);
    push(&mut out, &name.to_lowercase());
    for comp in name.split(['.', '_', '-']) {
        push(&mut out, comp);
        push(&mut out, &comp.to_lowercase());
    }
    out
}

/// A lazily-built, process-wide index of every icon file on disk, keyed by
/// lowercased file stem, keeping the best-looking variant per stem.
struct IconIndex {
    by_stem: HashMap<String, PathBuf>,
}

static ICON_INDEX: OnceLock<IconIndex> = OnceLock::new();

fn icon_index() -> &'static IconIndex {
    ICON_INDEX.get_or_init(IconIndex::build)
}

impl IconIndex {
    fn build() -> Self {
        let mut scored: HashMap<String, (u32, PathBuf)> = HashMap::new();
        for root in icon_roots() {
            walk_icons(&root, &mut scored);
        }
        let by_stem = scored.into_iter().map(|(k, (_, p))| (k, p)).collect();
        IconIndex { by_stem }
    }
}

/// Directories that may contain application icons.
fn icon_roots() -> Vec<PathBuf> {
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
    roots
}

/// Recursively collects icon files, keeping the best candidate per stem.
fn walk_icons(dir: &Path, scored: &mut HashMap<String, (u32, PathBuf)>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(ftype) = entry.file_type() else {
            continue;
        };
        if ftype.is_symlink() {
            continue;
        }
        if ftype.is_dir() {
            walk_icons(&path, scored);
        } else if ftype.is_file() {
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            let stem = stem.to_ascii_lowercase();
            if stem.is_empty() {
                continue;
            }
            let score = icon_score(&path);
            let slot = scored.entry(stem).or_insert((0, path.clone()));
            if score > slot.0 {
                slot.0 = score;
                slot.1 = path.clone();
            }
        }
    }
}

/// Prefer scalable vector icons, then larger rasters.
fn icon_score(path: &Path) -> u32 {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let kind = match ext.as_str() {
        "svg" => 2_000_000,
        "png" | "jpg" | "jpeg" | "webp" => 1_000_000,
        _ => 0,
    };
    let size = path
        .components()
        .filter_map(|c| {
            let s = c.as_os_str().to_string_lossy();
            if let Some((a, _)) = s.split_once('x') {
                a.parse::<u32>().ok()
            } else {
                s.parse::<u32>().ok()
            }
        })
        .max()
        .unwrap_or(0);
    kind + size
}
