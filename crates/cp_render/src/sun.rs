//! Day/night lighting as a pure function of [`GameTime`] and [`LightingSettings`].

use bevy::prelude::Resource;
use cp_core::GameTime;

/// Client-side lighting options (render only: the sim clock, citizens' routines and the UI
/// clock never see them).
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LightingSettings {
    /// Light the world as at noon whatever the game time is (no night).
    pub always_day: bool,
}

/// The fixed time of the always-day light, 15:00: a warm afternoon sun about 41° above the
/// horizon, low enough for long shadows and for a glint on the asphalt (T-178).
pub const ALWAYS_DAY_HOUR: f32 = 15.0;

fn afternoon() -> GameTime {
    GameTime {
        day: 0,
        tick_of_day: (ALWAYS_DAY_HOUR / 24.0 * cp_core::time::TICKS_PER_DAY as f32) as u32,
    }
}

/// [`sun_state`] of `time`, or of the fixed afternoon when `settings.always_day` is on.
pub fn lit_state(time: GameTime, settings: LightingSettings) -> SunState {
    sun_state(if settings.always_day {
        afternoon()
    } else {
        time
    })
}

/// Lighting for one instant. Colours are linear RGB.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SunState {
    /// Unit vector pointing from the ground towards the sun (x east, y up, z south).
    pub to_sun: [f32; 3],
    /// Unit vector the main light travels along (towards the ground). By night this is the
    /// moon's direction.
    pub light_dir: [f32; 3],
    /// Main light illuminance in lux (Bevy's `DirectionalLight::illuminance`).
    pub illuminance: f32,
    pub light_color: [f32; 3],
    /// Ambient light, cd/m² as in Bevy's `AmbientLight::brightness`.
    pub ambient_brightness: f32,
    pub ambient_color: [f32; 3],
    pub sky_color: [f32; 3],
}

fn smoothstep(a: f32, b: f32, v: f32) -> f32 {
    let t = ((v - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn lerp3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

fn norm(v: [f32; 3]) -> [f32; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    [v[0] / l, v[1] / l, v[2] / l]
}

/// Sun at sunrise 06:00 (east), zenith 12:00, sunset 18:00 (west); moon opposite.
pub fn sun_state(time: GameTime) -> SunState {
    let a = std::f32::consts::TAU * (time.day_fraction() - 0.25);
    let to_sun = norm([a.cos(), a.sin(), 0.4]);
    let elevation = to_sun[1];
    // Direct light: 0 once the sun is on the horizon, 1 in full daylight.
    let day = smoothstep(0.0, 0.25, elevation);
    // Ambient and sky keep glowing a little after sunset (twilight).
    let twilight = smoothstep(-0.15, 0.25, elevation);

    let sun_color = lerp3(
        [1.0, 0.55, 0.30],
        [1.0, 0.96, 0.90],
        smoothstep(0.0, 0.5, elevation),
    );
    let moon_color = [0.45, 0.55, 0.85];
    let to_moon = norm([-to_sun[0], -to_sun[1], to_sun[2]]);
    let main_dir = if elevation > 0.0 { to_sun } else { to_moon };

    let sun_lux = 10_000.0 * day;
    let moon_lux = 250.0 * (1.0 - day);
    let light_color = lerp3(moon_color, sun_color, day);
    let sky_day = lerp3(
        [0.60, 0.40, 0.30],
        [0.35, 0.55, 0.85],
        smoothstep(0.0, 0.4, elevation),
    );
    let sky_night = [0.01, 0.015, 0.04];

    SunState {
        to_sun: main_dir,
        light_dir: [-main_dir[0], -main_dir[1], -main_dir[2]],
        illuminance: sun_lux + moon_lux,
        light_color,
        ambient_brightness: 60.0 + 120.0 * twilight,
        ambient_color: lerp3([0.30, 0.38, 0.65], [0.75, 0.85, 1.0], twilight),
        sky_color: lerp3(sky_night, sky_day, twilight),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cp_core::Tick;
    use cp_core::time::TICKS_PER_DAY;

    fn at(hour: f32) -> SunState {
        sun_state(Tick((hour / 24.0 * TICKS_PER_DAY as f32) as u64).game_time())
    }

    #[test]
    fn always_day_is_the_afternoon_sun_at_any_time() {
        let on = LightingSettings { always_day: true };
        let off = LightingSettings::default();
        let noon = at(ALWAYS_DAY_HOUR);
        let elevation = noon.to_sun[1].asin().to_degrees();
        assert!((35.0..=45.0).contains(&elevation), "{elevation}°");
        assert_eq!(noon.illuminance, 10_000.0, "full daylight");
        for h in [0.0, 2.0, 5.5, 12.0, 18.5, 23.9] {
            let t = Tick((h / 24.0 * TICKS_PER_DAY as f32) as u64 + 3 * TICKS_PER_DAY).game_time();
            assert_eq!(lit_state(t, on), noon, "{h} h");
            assert_eq!(lit_state(t, off), sun_state(t), "{h} h, flag off");
        }
        assert_ne!(lit_state(at_time(2.0), off), noon);
    }

    fn at_time(hour: f32) -> GameTime {
        Tick((hour / 24.0 * TICKS_PER_DAY as f32) as u64).game_time()
    }

    #[test]
    fn noon_is_bright_and_overhead_midnight_is_dim() {
        let noon = at(12.0);
        let night = at(0.0);
        assert!(noon.to_sun[1] > 0.9, "sun high at noon: {:?}", noon.to_sun);
        assert_eq!(noon.illuminance, 10_000.0);
        assert!(night.illuminance < 500.0, "{}", night.illuminance);
        assert!(night.ambient_brightness < noon.ambient_brightness);
        assert!(night.sky_color[2] < noon.sky_color[2]);
    }

    #[test]
    fn sun_rises_east_and_sets_west() {
        assert!(at(6.5).to_sun[0] > 0.5);
        assert!(at(17.5).to_sun[0] < -0.5);
    }

    #[test]
    fn light_always_points_down_and_is_unit() {
        for h in 0..96 {
            let s = at(h as f32 / 4.0);
            assert!(
                s.light_dir[1] <= 1e-3,
                "hour {}: {:?}",
                h as f32 / 4.0,
                s.light_dir
            );
            let l: f32 = s.light_dir.iter().map(|v| v * v).sum();
            assert!((l - 1.0).abs() < 1e-4);
        }
    }

    #[test]
    fn lighting_is_continuous_over_the_day() {
        let mut prev = at(0.0);
        for k in 1..=2000 {
            let s = at(k as f32 * 24.0 / 2000.0);
            // Moon/sun handover swaps the light direction once (illuminance is tiny there).
            assert!((s.illuminance - prev.illuminance).abs() < 400.0, "k={k}");
            assert!(
                (s.ambient_brightness - prev.ambient_brightness).abs() < 5.0,
                "k={k}"
            );
            prev = s;
        }
    }
}
