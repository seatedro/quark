//! Reloading theme files while an app runs, for a development loop.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use super::file::{ThemeError, ThemeRegistry, load_file};

/// Watches a directory of theme files and reloads the ones that change.
///
/// It polls: call [`Self::poll`] from a timer (a few times a second is
/// plenty) or when the window regains focus. A file is reloaded when its
/// modification time or length changes. A file that fails to parse is
/// reported and leaves the family it last loaded in the registry, so a
/// half-typed edit never blanks the app's colors. Deleting a file keeps its
/// family too.
#[derive(Debug, Clone)]
pub struct ThemeWatcher {
    dir: PathBuf,
    /// Each file seen, with the stamp it had when last read.
    seen: Vec<(PathBuf, Stamp)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Stamp {
    modified: Option<SystemTime>,
    len: u64,
}

/// What a [`ThemeWatcher::poll`] changed.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ThemeReload {
    /// Families loaded or replaced, by name.
    pub loaded: Vec<String>,
    /// Files that changed but were rejected; their families stay as they
    /// were.
    pub errors: Vec<ThemeError>,
}

impl ThemeWatcher {
    /// Watch `dir`. The first poll loads every theme file in it.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            seen: Vec::new(),
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Reload the `*.json` files that changed since the last poll into
    /// `registry`, in name order. `None` when nothing changed.
    pub fn poll(&mut self, registry: &mut ThemeRegistry) -> Option<ThemeReload> {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return None;
        };
        let mut files: Vec<(PathBuf, Stamp)> = entries
            .filter_map(|entry| {
                let entry = entry.ok()?;
                let path = entry.path();
                if path.extension().is_none_or(|e| e != "json") {
                    return None;
                }
                let meta = entry.metadata().ok()?;
                let stamp = Stamp {
                    modified: meta.modified().ok(),
                    len: meta.len(),
                };
                Some((path, stamp))
            })
            .collect();
        files.sort_by(|a, b| a.0.cmp(&b.0));
        let mut reload = ThemeReload::default();
        let mut changed = false;
        for (path, stamp) in files {
            match self.seen.iter_mut().find(|(seen, _)| *seen == path) {
                Some((_, seen)) if *seen == stamp => continue,
                Some((_, seen)) => *seen = stamp,
                None => self.seen.push((path.clone(), stamp)),
            }
            changed = true;
            match load_file(&path) {
                Ok(family) => {
                    reload.loaded.push(family.name.clone());
                    registry.insert(family);
                }
                Err(error) => reload.errors.push(error),
            }
        }
        changed.then_some(reload)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{Color, ThemeMode};

    // Catches a reload that drops or blanks a theme on a broken edit, or
    // misses the edit that fixes it.
    #[test]
    fn a_broken_edit_keeps_the_last_valid_theme_until_it_is_fixed() {
        let dir = std::env::temp_dir().join(format!("quark-theme-watch-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let file = dir.join("iris.json");
        let write = |accent: &str| {
            let json =
                format!(r##"{{"name": "Iris", "dark": {{"colors": {{"accent": {accent}}}}}}}"##);
            std::fs::write(&file, json).expect("write");
        };
        let accent = |registry: &ThemeRegistry| {
            registry
                .get("Iris")
                .map(|f| f.theme(ThemeMode::Dark).colors.accent)
        };
        let mut registry = ThemeRegistry::default();
        let mut watcher = ThemeWatcher::new(&dir);

        write(r##""#ff0000""##);
        let first = watcher.poll(&mut registry).expect("loaded");
        assert_eq!(first.loaded, ["Iris"]);
        assert_eq!(watcher.poll(&mut registry), None, "unchanged");

        write(r##""#00ff"##);
        let broken = watcher.poll(&mut registry).expect("reported");
        assert_eq!((broken.loaded.len(), broken.errors.len()), (0, 1));
        assert_eq!(accent(&registry), Some(Color::rgba(255, 0, 0, 255)));

        write(r##""#0000ff""##);
        watcher.poll(&mut registry).expect("reloaded");
        std::fs::remove_dir_all(&dir).ok();
        assert_eq!(accent(&registry), Some(Color::rgba(0, 0, 255, 255)));
    }
}
