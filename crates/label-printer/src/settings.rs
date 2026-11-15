//! What the app remembers between runs, and the label presets it offers.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::content::PrepareOptions;

/// Key under which eframe stores [`Settings`].
pub const STORAGE_KEY: &str = "label-printer.settings";

/// Key used before the rename; read when the new one is missing.
pub const LEGACY_STORAGE_KEY: &str = "thermal-rs.settings";

/// A remembered Bluetooth LE printer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RememberedBle {
    /// Platform peripheral id.
    pub id: String,
    /// Advertised name, shown while reconnecting.
    pub name: String,
}

/// Everything persisted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Label width in millimetres.
    pub width_mm: f32,
    /// Label height in millimetres.
    pub height_mm: f32,
    /// Gap between labels; 0 = continuous media.
    pub gap_mm: f32,
    /// Print darkness 1-15.
    pub density: u8,
    /// Print speed.
    pub speed: u8,
    /// Copies per print.
    pub copies: u8,
    /// Crop, rotation, halftone, invert.
    pub options: PrepareOptions,
    /// Target spec of the last wired printer used (`usb:…`, `raw:…`, `serial:…`).
    pub last_printer: Option<String>,
    /// Last Bluetooth LE printer used.
    pub ble_printer: Option<RememberedBle>,
    /// Connect to the best printer on start and when one is plugged in.
    pub auto_connect: bool,
    /// Stream Bluetooth writes instead of waiting for each acknowledgement.
    pub ble_fast: bool,
    /// Folder the file picker opens in.
    pub last_dir: Option<PathBuf>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            width_mm: 101.6,
            height_mm: 152.4,
            gap_mm: 3.0,
            density: 8,
            speed: 4,
            copies: 1,
            options: PrepareOptions::default(),
            last_printer: None,
            ble_printer: None,
            auto_connect: true,
            ble_fast: false,
            last_dir: None,
        }
    }
}

/// A common label stock.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Preset {
    /// Name in the picker.
    pub name: &'static str,
    /// Width in millimetres.
    pub width_mm: f32,
    /// Height in millimetres.
    pub height_mm: f32,
    /// Gap in millimetres.
    pub gap_mm: f32,
}

/// Label sizes offered in the picker.
pub const PRESETS: &[Preset] = &[
    Preset {
        name: "4 × 6 in shipping",
        width_mm: 101.6,
        height_mm: 152.4,
        gap_mm: 3.0,
    },
    Preset {
        name: "100 × 150 mm",
        width_mm: 100.0,
        height_mm: 150.0,
        gap_mm: 3.0,
    },
    Preset {
        name: "4 × 4 in",
        width_mm: 101.6,
        height_mm: 101.6,
        gap_mm: 3.0,
    },
    Preset {
        name: "4 × 3 in",
        width_mm: 101.6,
        height_mm: 76.2,
        gap_mm: 3.0,
    },
    Preset {
        name: "4 × 2 in",
        width_mm: 101.6,
        height_mm: 50.8,
        gap_mm: 3.0,
    },
    Preset {
        name: "3 × 2 in",
        width_mm: 76.2,
        height_mm: 50.8,
        gap_mm: 3.0,
    },
    Preset {
        name: "2.25 × 1.25 in",
        width_mm: 57.15,
        height_mm: 31.75,
        gap_mm: 3.0,
    },
    Preset {
        name: "2 × 1 in",
        width_mm: 50.8,
        height_mm: 25.4,
        gap_mm: 3.0,
    },
];

impl Settings {
    /// The preset matching the current size, if any.
    #[must_use]
    pub fn preset(&self) -> Option<&'static Preset> {
        PRESETS.iter().find(|preset| {
            (preset.width_mm - self.width_mm).abs() < 0.05
                && (preset.height_mm - self.height_mm).abs() < 0.05
        })
    }

    /// Clamps values a hand-edited or older settings file may have broken.
    #[must_use]
    pub fn sanitized(mut self) -> Self {
        let defaults = Self::default();
        if !(5.0..=108.0).contains(&self.width_mm) {
            self.width_mm = defaults.width_mm;
        }
        if !(5.0..=1000.0).contains(&self.height_mm) {
            self.height_mm = defaults.height_mm;
        }
        if !(0.0..=20.0).contains(&self.gap_mm) {
            self.gap_mm = defaults.gap_mm;
        }
        self.density = self.density.clamp(1, 15);
        self.speed = self.speed.clamp(1, 12);
        self.copies = self.copies.clamp(1, 99);
        self
    }
}

/// Folder name used under the per-user directories and beside the executable.
pub const APP_DIR: &str = "label-printer";

/// Folder name used before the app was renamed; settings are copied from it.
pub const LEGACY_APP_DIR: &str = "thermal-rs";

/// Per-user data directory (`%LOCALAPPDATA%\label-printer` on Windows).
#[must_use]
pub fn data_dir() -> Option<PathBuf> {
    user_data_base().map(|base| base.join(APP_DIR))
}

fn user_data_base() -> Option<PathBuf> {
    if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|home| {
            PathBuf::from(home)
                .join("Library")
                .join("Application Support")
        })
    } else {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME")
                    .map(|home| PathBuf::from(home).join(".local").join("share"))
            })
    }
}

/// Millimetres to printer dots, at least one.
#[must_use]
pub fn mm_to_dots(mm: f32, dpi: u32) -> u32 {
    let dots = (f64::from(mm) * f64::from(dpi) / 25.4).round();
    if dots.is_finite() && dots >= 1.0 {
        dots as u32
    } else {
        1
    }
}

/// Where the settings file lives, and whether that is the normal location.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageLocation {
    /// The `app.ron` file eframe reads and writes.
    pub file: PathBuf,
    /// False when the normal per-user folder was not writable and a fallback
    /// (beside the executable, or the temp folder) is used instead.
    pub preferred: bool,
}

/// Picks a settings file that can actually be written.
///
/// eframe's own default disables saving without a word when its folder cannot
/// be created, which made the app forget the Bluetooth printer on every start.
/// The candidates are probed by writing to them: the per-user folder, a
/// `label-printer-data` folder beside the executable (portable use), then the
/// temp folder. Settings saved under the app's old name are copied over once.
#[must_use]
pub fn storage_location() -> Option<StorageLocation> {
    let preferred = config_dir_named(APP_DIR).map(|dir| dir.join("data"));
    let candidates = storage_candidates(APP_DIR);
    let dir = choose_dir(&candidates)?;
    migrate_legacy(&dir, &storage_candidates(LEGACY_APP_DIR));
    Some(StorageLocation {
        preferred: preferred.as_ref() == Some(&dir),
        file: dir.join("app.ron"),
    })
}

/// Settings folders for an app folder name, in order of preference.
fn storage_candidates(name: &str) -> Vec<PathBuf> {
    let preferred = config_dir_named(name).map(|dir| dir.join("data"));
    let beside_exe = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(format!("{name}-data"))));
    let temp = Some(std::env::temp_dir().join(format!("{name}-data")));
    [preferred, beside_exe, temp]
        .into_iter()
        .flatten()
        .collect()
}

/// Copies `app.ron` from the first legacy folder that has one into `target`,
/// unless `target` already has settings. Returns the file copied from.
pub fn migrate_legacy(target: &std::path::Path, legacy: &[PathBuf]) -> Option<PathBuf> {
    let destination = target.join("app.ron");
    if destination.is_file() {
        return None;
    }
    let source = legacy
        .iter()
        .map(|dir| dir.join("app.ron"))
        .find(|file| file.is_file())?;
    std::fs::copy(&source, &destination).ok()?;
    Some(source)
}

/// A writable candidate that already holds settings, so they are never split
/// between two places; otherwise the first writable one.
#[must_use]
pub fn choose_dir(candidates: &[PathBuf]) -> Option<PathBuf> {
    candidates
        .iter()
        .find(|dir| dir.join("app.ron").is_file() && is_writable(dir))
        .cloned()
        .or_else(|| first_writable(candidates))
}

/// The first directory that can be created and written to.
#[must_use]
pub fn first_writable(candidates: &[PathBuf]) -> Option<PathBuf> {
    candidates.iter().find(|dir| is_writable(dir)).cloned()
}

fn is_writable(dir: &std::path::Path) -> bool {
    if std::fs::create_dir_all(dir).is_err() {
        return false;
    }
    let probe = dir.join(".write-probe");
    let ok = std::fs::write(&probe, b"ok").is_ok();
    let _ = std::fs::remove_file(&probe);
    ok
}

/// Per-user settings directory for an app folder name
/// (`%APPDATA%\label-printer` on Windows).
fn config_dir_named(name: &str) -> Option<PathBuf> {
    if cfg!(windows) {
        std::env::var_os("APPDATA").map(|base| PathBuf::from(base).join(name))
    } else {
        user_data_base().map(|base| base.join(name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_are_recognised() {
        let settings = Settings::default();
        assert_eq!(settings.preset().map(|p| p.name), Some("4 × 6 in shipping"));
        let custom = Settings {
            width_mm: 70.0,
            ..Settings::default()
        };
        assert!(custom.preset().is_none());
    }

    #[test]
    fn sanitizing_repairs_out_of_range_values() {
        let broken = Settings {
            width_mm: 500.0,
            height_mm: -3.0,
            gap_mm: f32::NAN,
            density: 0,
            speed: 99,
            copies: 0,
            ..Settings::default()
        }
        .sanitized();
        let defaults = Settings::default();
        assert_eq!(broken.width_mm, defaults.width_mm);
        assert_eq!(broken.height_mm, defaults.height_mm);
        assert_eq!(broken.gap_mm, defaults.gap_mm);
        assert_eq!((broken.density, broken.speed, broken.copies), (1, 12, 1));

        assert_eq!(defaults.clone().sanitized(), defaults);
    }

    #[test]
    fn millimetres_become_dots() {
        assert_eq!(mm_to_dots(101.6, 203), 812);
        assert_eq!(mm_to_dots(152.4, 203), 1218);
        assert_eq!(mm_to_dots(0.0, 203), 1);
        assert_eq!(mm_to_dots(f32::NAN, 203), 1);
    }

    #[test]
    fn settings_round_trip_and_tolerate_missing_fields() {
        // eframe stores settings as RON. An older file with fields missing
        // still loads, with defaults for the rest.
        let old: Settings = ron::from_str("(density: 12)").expect("parse");
        assert_eq!(old.density, 12);
        assert!(old.auto_connect);

        let text = ron::to_string(&Settings::default()).expect("serialise");
        let back: Settings = ron::from_str(&text).expect("parse back");
        assert_eq!(back, Settings::default());

        assert!(ron::from_str::<Settings>("(density: \"dark\")").is_err());
    }

    #[test]
    fn the_first_writable_folder_wins() {
        let base = std::env::current_dir()
            .expect("cwd")
            .join("target")
            .join(format!("settings-probe-{}", std::process::id()));
        // A path under an existing *file* can never be created.
        let blocker = base.join("blocker");
        std::fs::create_dir_all(&base).expect("base");
        std::fs::write(&blocker, b"file").expect("blocker");

        let unwritable = blocker.join("data");
        let writable = base.join("data");
        assert_eq!(
            first_writable(&[unwritable.clone(), writable.clone()]),
            Some(writable.clone())
        );
        assert!(writable.is_dir());
        assert!(
            !writable.join(".write-probe").exists(),
            "probe file cleaned up"
        );
        assert_eq!(first_writable(&[unwritable]), None);
        assert_eq!(first_writable(&[]), None);

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn existing_settings_are_kept_where_they_are() {
        let base = std::env::current_dir()
            .expect("cwd")
            .join("target")
            .join(format!("settings-choose-{}", std::process::id()));
        let usual = base.join("usual");
        let fallback = base.join("fallback");
        std::fs::create_dir_all(&fallback).expect("fallback");

        // Nothing saved yet: the usual place wins.
        assert_eq!(
            choose_dir(&[usual.clone(), fallback.clone()]),
            Some(usual.clone())
        );

        // Settings already in the fallback: keep using them.
        std::fs::write(fallback.join("app.ron"), "{}").expect("settings");
        assert_eq!(
            choose_dir(&[usual.clone(), fallback.clone()]),
            Some(fallback)
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn legacy_settings_are_copied_once() {
        let base = std::env::current_dir()
            .expect("cwd")
            .join("target")
            .join(format!("settings-migrate-{}", std::process::id()));
        let new = base.join("new");
        let old_missing = base.join("old-missing");
        let old = base.join("old");
        std::fs::create_dir_all(&new).expect("new");
        std::fs::create_dir_all(&old).expect("old");
        std::fs::write(old.join("app.ron"), "old settings").expect("old file");

        // Nothing in the new place: copied from the first legacy folder that has a file.
        let copied = migrate_legacy(&new, &[old_missing.clone(), old.clone()]);
        assert_eq!(copied, Some(old.join("app.ron")));
        assert_eq!(
            std::fs::read_to_string(new.join("app.ron")).expect("copied"),
            "old settings"
        );

        // Already migrated (or newer settings saved): never overwritten.
        std::fs::write(new.join("app.ron"), "new settings").expect("new file");
        assert_eq!(migrate_legacy(&new, std::slice::from_ref(&old)), None);
        assert_eq!(
            std::fs::read_to_string(new.join("app.ron")).expect("kept"),
            "new settings"
        );

        // No legacy settings anywhere.
        assert_eq!(migrate_legacy(&base.join("empty"), &[old_missing]), None);

        let _ = std::fs::remove_dir_all(&base);
    }
}
