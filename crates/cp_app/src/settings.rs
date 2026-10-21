//! The player's settings file (T-069): `<data dir>/settings.ron`, read at start and written
//! when something changes. Every field has a default, so a missing file, a missing or unknown
//! field or a broken file never stops the game: a broken file logs a warning and gives the
//! defaults. Not sim state; nothing here touches determinism.

use std::path::{Path, PathBuf};

use bevy::prelude::*;
use cp_render::{CameraSettings, GradingSettings, LightingSettings};
use cp_ui::UiState;
use serde::{Deserialize, Serialize};

/// File name inside the data directory.
pub const FILE: &str = "settings.ron";
/// Shortest time between two writes while a slider is dragged, seconds.
const WRITE_EVERY: f64 = 0.5;

/// The camera sensitivities that are saved.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CameraPrefs {
    pub pan_speed: f32,
    pub key_rotate_speed: f32,
    pub key_tilt_speed: f32,
    pub drag_rotate_sens: f32,
    pub zoom_step: f32,
    pub edge_pan: bool,
}

impl Default for CameraPrefs {
    fn default() -> Self {
        let c = CameraSettings::default();
        Self {
            pan_speed: c.pan_speed,
            key_rotate_speed: c.key_rotate_speed,
            key_tilt_speed: c.key_tilt_speed,
            drag_rotate_sens: c.drag_rotate_sens,
            zoom_step: c.zoom_step,
            edge_pan: c.edge_pan,
        }
    }
}

/// The colour grading the player tuned (T-273); the defaults change nothing.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GradingPrefs {
    pub exposure: f32,
    pub contrast: f32,
    pub saturation: f32,
    pub temperature: f32,
    pub tint: f32,
    pub shadows_lift: f32,
    pub highlights_gain: f32,
    pub vignette: f32,
}

impl Default for GradingPrefs {
    fn default() -> Self {
        Self::from(GradingSettings::NEUTRAL)
    }
}

impl From<GradingSettings> for GradingPrefs {
    fn from(g: GradingSettings) -> Self {
        Self {
            exposure: g.exposure,
            contrast: g.contrast,
            saturation: g.saturation,
            temperature: g.temperature,
            tint: g.tint,
            shadows_lift: g.shadows_lift,
            highlights_gain: g.highlights_gain,
            vignette: g.vignette,
        }
    }
}

impl GradingPrefs {
    /// The grading resource, every value clamped to its slider's range (a hand-edited file).
    pub fn settings(&self) -> GradingSettings {
        let mut g = GradingSettings {
            exposure: self.exposure,
            contrast: self.contrast,
            saturation: self.saturation,
            temperature: self.temperature,
            tint: self.tint,
            shadows_lift: self.shadows_lift,
            highlights_gain: self.highlights_gain,
            vignette: self.vignette,
        };
        let mut neutral = GradingSettings::NEUTRAL;
        for ((_, v, range), (_, d, _)) in g.fields().into_iter().zip(neutral.fields()) {
            *v = if v.is_finite() {
                v.clamp(*range.start(), *range.end())
            } else {
                *d
            };
        }
        g
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub always_day: bool,
    /// The shadows of passing clouds (T-272).
    pub cloud_shadows: bool,
    /// Parked cars along the kerbs (T-276).
    pub parked_cars: bool,
    pub ui_scale: f32,
    pub camera: CameraPrefs,
    /// The city advisor snackbar (T-188) and whether it only names problems.
    pub advisor: bool,
    pub advisor_problems_only: bool,
    /// Sound volumes and mute (T-189).
    pub audio: crate::audio::AudioSettings,
    /// Colour grading (T-273).
    pub grading: GradingPrefs,
    /// The language of the UI, `"en"` or `"ru"` (T-284); English until the user decides.
    pub language: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            // T-178: the world is lit by the sun all the time unless the player turns the
            // day and night cycle back on.
            always_day: true,
            cloud_shadows: true,
            parked_cars: true,
            ui_scale: 1.0,
            camera: CameraPrefs::default(),
            advisor: true,
            advisor_problems_only: false,
            audio: crate::audio::AudioSettings::default(),
            grading: GradingPrefs::default(),
            language: cp_locale::Lang::En.code().to_string(),
        }
    }
}

impl Settings {
    pub fn path(data: &Path) -> PathBuf {
        data.join(FILE)
    }

    /// Reads `settings.ron` in `data`: defaults when the file is missing, defaults and a
    /// warning when it cannot be parsed. Out-of-range values are clamped.
    pub fn load(data: &Path) -> Self {
        let path = Self::path(data);
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Self::default(),
            Err(e) => {
                warn!(
                    "settings: cannot read {}: {e}; using defaults",
                    path.display()
                );
                return Self::default();
            }
        };
        match ron::from_str::<Self>(&text) {
            Ok(s) => s.sanitized(),
            Err(e) => {
                warn!(
                    "settings: {} is broken ({e}); using defaults",
                    path.display()
                );
                Self::default()
            }
        }
    }

    /// Writes the file (via a temporary file, so a crash never leaves half of it).
    pub fn save(&self, data: &Path) -> Result<(), String> {
        let text = ron::ser::to_string_pretty(self, ron::ser::PrettyConfig::default())
            .map_err(|e| e.to_string())?;
        std::fs::create_dir_all(data).map_err(|e| e.to_string())?;
        let path = Self::path(data);
        let tmp = path.with_extension("ron.tmp");
        std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &path).map_err(|e| e.to_string())
    }

    /// Keeps hand-edited values in a range the game works with.
    fn sanitized(mut self) -> Self {
        let fix = |v: &mut f32, lo: f32, hi: f32, default: f32| {
            *v = if v.is_finite() {
                v.clamp(lo, hi)
            } else {
                default
            };
        };
        fix(
            &mut self.ui_scale,
            *cp_ui::UI_SCALE_RANGE.start(),
            *cp_ui::UI_SCALE_RANGE.end(),
            1.0,
        );
        let d = CameraPrefs::default();
        let c = &mut self.camera;
        fix(&mut c.pan_speed, 0.05, 10.0, d.pan_speed);
        fix(&mut c.key_rotate_speed, 0.1, 10.0, d.key_rotate_speed);
        fix(&mut c.key_tilt_speed, 0.1, 10.0, d.key_tilt_speed);
        fix(&mut c.drag_rotate_sens, 0.0005, 0.05, d.drag_rotate_sens);
        fix(&mut c.zoom_step, 0.5, 0.98, d.zoom_step);
        self.audio = self.audio.sanitized();
        self
    }

    /// The camera resource these settings describe (the rest stays at its default).
    pub fn camera_settings(&self) -> CameraSettings {
        let c = &self.camera;
        CameraSettings {
            pan_speed: c.pan_speed,
            key_rotate_speed: c.key_rotate_speed,
            key_tilt_speed: c.key_tilt_speed,
            drag_rotate_sens: c.drag_rotate_sens,
            zoom_step: c.zoom_step,
            edge_pan: c.edge_pan,
            ..CameraSettings::default()
        }
    }

    fn current(
        lighting: &LightingSettings,
        camera: &CameraSettings,
        ui: &UiState,
        grading: &GradingSettings,
    ) -> Self {
        Self {
            always_day: lighting.always_day,
            cloud_shadows: lighting.cloud_shadows,
            parked_cars: lighting.parked_cars,
            ui_scale: ui.prefs.ui_scale,
            camera: CameraPrefs {
                pan_speed: camera.pan_speed,
                key_rotate_speed: camera.key_rotate_speed,
                key_tilt_speed: camera.key_tilt_speed,
                drag_rotate_sens: camera.drag_rotate_sens,
                zoom_step: camera.zoom_step,
                edge_pan: camera.edge_pan,
            },
            advisor: ui.prefs.advisor,
            advisor_problems_only: ui.prefs.advisor_problems_only,
            audio: crate::audio::AudioSettings {
                master: ui.prefs.audio.master,
                effects: ui.prefs.audio.effects,
                mute: ui.prefs.audio.mute,
            },
            grading: GradingPrefs::from(*grading),
            language: ui.prefs.lang.code().to_string(),
        }
    }
}

/// What is on disk (or was last written), and when.
#[derive(Resource)]
pub struct SettingsSaver {
    saved: Settings,
    last_write: f64,
    /// A hidden run (screenshots) never writes the player's file.
    read_only: bool,
}

impl SettingsSaver {
    pub fn new(loaded: Settings, read_only: bool) -> Self {
        Self {
            saved: loaded,
            last_write: f64::MIN,
            read_only,
        }
    }
}

/// Writes the settings when they differ from the file, at most every [`WRITE_EVERY`] seconds.
pub fn save_settings(
    time: Res<Time<Real>>,
    lighting: Res<LightingSettings>,
    camera: Res<CameraSettings>,
    ui: Res<UiState>,
    grading: Res<GradingSettings>,
    mut saver: ResMut<SettingsSaver>,
) {
    if saver.read_only {
        return;
    }
    let now = Settings::current(&lighting, &camera, &ui, &grading);
    let t = time.elapsed_secs_f64();
    if now == saver.saved || t - saver.last_write < WRITE_EVERY {
        return;
    }
    saver.last_write = t;
    match now.save(&super::saves::data_dir()) {
        Ok(()) => saver.saved = now,
        Err(e) => warn!("settings: cannot write {FILE}: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("cp_settings_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn settings_round_trip_through_the_file() {
        let d = dir("round");
        let s = Settings {
            always_day: true,
            cloud_shadows: false,
            parked_cars: false,
            ui_scale: 1.5,
            camera: CameraPrefs {
                pan_speed: 1.7,
                edge_pan: false,
                ..CameraPrefs::default()
            },
            advisor: false,
            advisor_problems_only: false,
            audio: crate::audio::AudioSettings {
                master: 0.5,
                effects: 0.25,
                mute: true,
            },
            language: "ru".to_string(),
            grading: GradingPrefs {
                contrast: 1.2,
                temperature: -0.1,
                ..GradingPrefs::default()
            },
        };
        s.save(&d).unwrap();
        assert_eq!(Settings::load(&d), s);
    }

    #[test]
    fn missing_file_missing_and_unknown_fields_and_a_broken_file_are_safe() {
        let d = dir("odd");
        assert_eq!(Settings::load(&d), Settings::default(), "no file");
        let write = |t: &str| std::fs::write(Settings::path(&d), t).unwrap();
        write("(always_day: true, colour_of_the_sky: 7, camera: (pan_speed: 2.0, wat: 1))");
        let s = Settings::load(&d);
        assert!(s.always_day);
        assert_eq!(s.camera.pan_speed, 2.0);
        assert_eq!(s.ui_scale, 1.0, "missing field = default");
        write("(always_day: tru");
        assert_eq!(Settings::load(&d), Settings::default(), "broken file");
        write("(ui_scale: 99.0)");
        assert_eq!(Settings::load(&d).ui_scale, *cp_ui::UI_SCALE_RANGE.end());
    }

    /// T-178: the sun is on all day unless the player turns the cycle back on.
    #[test]
    fn always_day_is_the_default() {
        assert!(Settings::default().always_day);
        let d = dir("default_day");
        assert!(Settings::load(&d).always_day, "no file: always day");
    }
}
