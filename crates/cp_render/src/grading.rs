//! The colour grading of the camera (T-273, the user's "filters"): exposure, contrast,
//! saturation, warm/blue temperature, tint, the lift of the shadows, the gain of the highlights
//! and a vignette, applied live to every 3D camera from [`GradingSettings`]. The defaults change
//! nothing: the picture is the one before T-273 until the player moves a slider.

use bevy::post_process::effect_stack::Vignette;
use bevy::prelude::*;
use bevy::render::view::ColorGrading;

/// The grading the player tuned (client only; saved in the settings file by the app).
#[derive(Resource, Clone, Copy, Debug, PartialEq)]
pub struct GradingSettings {
    /// Exposure offset, stops.
    pub exposure: f32,
    /// Contrast of the shadows, midtones and highlights (1 = unchanged).
    pub contrast: f32,
    /// Saturation after tonemapping (1 = unchanged, 0 = grey).
    pub saturation: f32,
    /// Warm (positive) or blue (negative): the CIE x shift.
    pub temperature: f32,
    /// Magenta (positive) or green (negative): the CIE y shift.
    pub tint: f32,
    /// Lift of the shadows (0 = unchanged; positive brightens the darks).
    pub shadows_lift: f32,
    /// Gain of the highlights (1 = unchanged).
    pub highlights_gain: f32,
    /// Darkening of the corners (0 = none, 1 = black corners).
    pub vignette: f32,
}

impl Default for GradingSettings {
    fn default() -> Self {
        Self::NEUTRAL
    }
}

impl GradingSettings {
    /// No change to the picture.
    pub const NEUTRAL: Self = Self {
        exposure: 0.0,
        contrast: 1.0,
        saturation: 1.0,
        temperature: 0.0,
        tint: 0.0,
        shadows_lift: 0.0,
        highlights_gain: 1.0,
        vignette: 0.0,
    };

    /// The fields with their names and the slider ranges, in panel order.
    pub fn fields(&mut self) -> [(&'static str, &mut f32, std::ops::RangeInclusive<f32>); 8] {
        [
            ("exposure", &mut self.exposure, -2.0..=2.0),
            ("contrast", &mut self.contrast, 0.5..=1.5),
            ("saturation", &mut self.saturation, 0.0..=2.0),
            ("temperature", &mut self.temperature, -0.5..=0.5),
            ("tint", &mut self.tint, -0.5..=0.5),
            ("shadows_lift", &mut self.shadows_lift, -0.2..=0.2),
            ("highlights_gain", &mut self.highlights_gain, 0.5..=1.5),
            ("vignette", &mut self.vignette, 0.0..=1.0),
        ]
    }

    /// The values as one line, `name=value` comma separated (what "copy values" gives and
    /// what [`Self::parse`] reads back, e.g. from `--grading`).
    pub fn to_line(mut self) -> String {
        self.fields()
            .iter()
            .map(|(name, v, _)| format!("{name}={:.3}", **v))
            .collect::<Vec<_>>()
            .join(",")
    }

    /// Reads [`Self::to_line`]'s format over the neutral values; unknown names and bad
    /// numbers are errors.
    pub fn parse(line: &str) -> Result<Self, String> {
        let mut out = Self::NEUTRAL;
        for pair in line.split(',').map(str::trim).filter(|p| !p.is_empty()) {
            let (name, value) = pair
                .split_once('=')
                .ok_or_else(|| format!("`{pair}`: expected name=value"))?;
            let value: f32 = value
                .trim()
                .parse()
                .map_err(|_| format!("`{pair}`: not a number"))?;
            let mut fields = out.fields();
            let field = fields
                .iter_mut()
                .find(|(n, ..)| *n == name.trim())
                .ok_or_else(|| format!("`{name}`: no such grading value"))?;
            *field.1 = value.clamp(*field.2.start(), *field.2.end());
        }
        Ok(out)
    }

    /// The camera's colour grading for these settings.
    pub fn color_grading(&self) -> ColorGrading {
        let mut g = ColorGrading::default();
        g.global.exposure = self.exposure;
        g.global.temperature = self.temperature;
        g.global.tint = self.tint;
        g.global.post_saturation = self.saturation;
        for section in [&mut g.shadows, &mut g.midtones, &mut g.highlights] {
            section.contrast = self.contrast;
        }
        g.shadows.lift = self.shadows_lift;
        g.highlights.gain = self.highlights_gain;
        g
    }
}

pub struct GradingPlugin;

impl Plugin for GradingPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<GradingSettings>()
            .add_systems(PostUpdate, apply_grading);
    }
}

/// Puts the settings on every 3D camera when they change (and on a camera that has just
/// appeared). A vignette of 0 is no vignette component at all.
fn apply_grading(
    settings: Res<GradingSettings>,
    mut commands: Commands,
    cameras: Query<(Entity, Ref<Camera3d>), With<Camera3d>>,
) {
    for (entity, cam) in &cameras {
        if !settings.is_changed() && !cam.is_added() {
            continue;
        }
        let mut e = commands.entity(entity);
        e.insert(settings.color_grading());
        if settings.vignette > 0.0 {
            e.insert(Vignette {
                intensity: settings.vignette,
                ..default()
            });
        } else {
            e.remove::<Vignette>();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_neutral_settings_are_the_default_grading() {
        assert_eq!(
            GradingSettings::NEUTRAL.color_grading(),
            ColorGrading::default()
        );
    }

    #[test]
    fn the_copied_line_reads_back() {
        let s = GradingSettings {
            contrast: 1.15,
            temperature: -0.08,
            vignette: 0.3,
            ..GradingSettings::NEUTRAL
        };
        assert_eq!(GradingSettings::parse(&s.to_line()), Ok(s));
        assert_eq!(
            GradingSettings::parse("contrast=9"),
            Ok(GradingSettings {
                contrast: 1.5,
                ..GradingSettings::NEUTRAL
            }),
            "clamped to the slider's range"
        );
        assert!(GradingSettings::parse("glow=1").is_err());
    }
}
