//! Lit windows at night (T-245), as pure functions.
//!
//! Which windows light up is a hash of the building's seed and the window's index, baked into
//! the mesh: the *vertex alpha* of a glass quad carries a glow code (see [`glow_alpha`]). The
//! building material is opaque, so the alpha is free; a plain surface keeps alpha 1. Two
//! numbers that change with time go to the material as one uniform: the night factor (the
//! lamps' `night_factor`, so windows fade in at dusk and out at dawn with the street lights)
//! and the [`lateness`] of the night. Each lit window goes dark once the night is later than
//! its own switch-off time, so the city empties towards morning without a mesh rebuild.
//! `vertex_color.wesl` decodes the code exactly as [`glow`] does.
//!
//! A building that is not active (under construction, abandoned, burned) or has no power
//! (`BuildingFlags::NO_POWER`) shows no light at all: an outage reads at night.

use cp_building::{Building, BuildingFlags, BuildingState};
use cp_content::ZoneType;
use cp_core::GameTime;

/// How a building's windows behave at night.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LightClass {
    /// Homes: about half the windows lit in the evening, fewer after midnight.
    Home,
    /// Offices, shops and services: lit in the evening, mostly dark late.
    Work,
    /// Industry: a few windows, most of them lit all night (shifts).
    Industry,
}

/// The light class of a building of `zone`, or `None` when it shows no light (not active, or
/// unpowered). A city service (`service`; zoned industrial in content) lights like an office
/// (T-248).
pub fn light_class(b: &Building, zone: ZoneType, service: bool) -> Option<LightClass> {
    if b.state != BuildingState::Active || b.flags.has(BuildingFlags::NO_POWER) {
        return None;
    }
    if service {
        return Some(LightClass::Work);
    }
    Some(match zone {
        ZoneType::ResLow | ZoneType::ResHigh => LightClass::Home,
        ZoneType::Industrial => LightClass::Industry,
        ZoneType::ComLow | ZoneType::ComHigh | ZoneType::Office | ZoneType::None => {
            LightClass::Work
        }
    })
}

/// How late in the night `time` is: 0 until 19:00, rising evenly to 1 at 04:00, 1 until noon
/// (the jump back to 0 happens at noon, in full daylight, where no window glows).
pub fn lateness(time: GameTime) -> f32 {
    let hour = time.day_fraction() * 24.0;
    let since_noon = (hour - 12.0).rem_euclid(24.0);
    ((since_noon - 7.0) / 9.0).clamp(0.0, 1.0)
}

/// Lamp colours of lit windows, linear RGB: 2700 K, 3000 K, 3500 K and a cooler white (TV,
/// LED). Index = [`WindowLight::tint`]. `vertex_color.wesl` holds the same table.
pub const TINTS: [[f32; 3]; 4] = [
    [1.0, 0.50, 0.18],
    [1.0, 0.60, 0.28],
    [1.0, 0.72, 0.42],
    [0.78, 0.85, 1.0],
];

/// One window that is lit tonight.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WindowLight {
    /// [`lateness`] at which it goes dark; `None` = lit until dawn.
    pub off: Option<f32>,
    /// Index into [`TINTS`].
    pub tint: u8,
}

fn mix(seed: u32, a: u32, b: u32) -> u32 {
    let mut h = seed ^ a.wrapping_mul(0x9e37_79b9) ^ b.wrapping_mul(0x85eb_ca6b);
    h ^= h >> 16;
    h = h.wrapping_mul(0x7feb_352d);
    h ^= h >> 15;
    h = h.wrapping_mul(0x846c_a68b);
    h ^= h >> 16;
    h
}

fn unit(h: u32) -> f32 {
    (h >> 8) as f32 / (1u32 << 24) as f32
}

/// Share of a building's windows lit in the evening (before any goes dark), from its seed.
pub fn evening_share(class: LightClass, seed: u32) -> f32 {
    let (lo, hi) = match class {
        LightClass::Home => (0.35, 0.60),
        LightClass::Work => (0.55, 0.80),
        LightClass::Industry => (0.10, 0.20),
    };
    lo + (hi - lo) * unit(mix(seed, 0x51_6874, 0))
}

/// When the lit windows of `class` go dark: the share that stays on until dawn, and the
/// [`lateness`] range (start, length) the others switch off in, evenly. `vertex_color.wesl`
/// holds the same table for the window patterns.
fn switch_off(class: LightClass) -> (f32, f32, f32) {
    match class {
        LightClass::Home => (0.22, 0.15, 0.85),
        LightClass::Work => (0.08, 0.02, 0.40),
        LightClass::Industry => (0.70, 0.0, 1.0),
    }
}

/// Whether window `window` of the building with `seed` is lit tonight, and how: a stable
/// function of the two (no RNG, no time).
pub fn window_light(class: LightClass, seed: u32, window: u32) -> Option<WindowLight> {
    let h = mix(seed, window, 0x6c_6974);
    if unit(h) >= evening_share(class, seed) {
        return None;
    }
    let g = mix(seed, window, 0x6f_6666);
    let u = unit(g);
    let (all_night, start, span) = switch_off(class);
    // The rest spread over the night.
    let off = (u >= all_night).then(|| start + span * (u - all_night) / (1.0 - all_night));
    let tint = match g & 0xff {
        0..=89 => 0,
        90..=166 => 1,
        167..=217 => 2,
        _ => 3,
    };
    Some(WindowLight { off, tint })
}

/// Glow codes in the vertex alpha: `2 + code`, a whole number below 2^24, so exact in `f32`;
/// the shader rounds it. Two kinds:
///
/// * a **window** ([`glow_alpha`]): off (6 bits, 63 = all night) | tint (2 bits) << 6 |
///   strength (5 bits, the square root of the strength in 31sts) << 8;
/// * a **pattern** ([`pattern_alpha`], T-246) on a surface that stands for many windows (a
///   floor's glass band, a whole wall far away): share (5 bits, 31sts) | class (2 bits) << 5 |
///   band (1 bit) << 7 | bay (4 bits, half metres) << 8 | floor height (4 bits, half metres)
///   << 12 | window width (4 bits, 15ths of the bay) << 16 | 1 << 20. The shader draws a grid
///   of windows over the surface in world space, lights each cell from a hash of it and
///   switches it off as [`window_light`] would; where a pixel covers a whole cell it shows
///   the cells' average ([`glow`] of the code). So a distant city sparkles with single
///   windows instead of glowing as flat bands.
const ALL_NIGHT: u32 = 63;
const PATTERN: u32 = 1 << 20;

/// Window height of a pattern cell on a wall, as a share of the floor (the band is all
/// window). The shader's `0.3 .. 0.8`.
const PATTERN_WIN_H: f32 = 0.5;

/// Vertex alpha of a lit window: `strength` its glow (1 = full), `off` its switch-off time.
/// The strength's square root rounds to 1/31; a zero strength is a plain surface (alpha 1).
pub fn glow_alpha(off: Option<f32>, tint: u8, strength: f32) -> f32 {
    let s = (strength.clamp(0.0, 1.0).sqrt() * 31.0).round() as u32;
    if s == 0 {
        return 1.0;
    }
    let q = off.map_or(ALL_NIGHT, |o| (o.clamp(0.0, 1.0) * 62.0).round() as u32);
    let code = q | (u32::from(tint) & 3) << 6 | s << 8;
    2.0 + code as f32
}

/// A grid of windows the shader draws on a surface standing for many of them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pattern {
    pub class: LightClass,
    /// Share of the cells lit in the evening.
    pub share: f32,
    /// A floor's glass band (cells as tall as the surface's floor, the window fills it), or a
    /// wall (cells of `floor` metres, the window in the middle half).
    pub band: bool,
    /// Width of a cell, metres (1 .. 7.5).
    pub bay: f32,
    /// Height of a cell, metres (2 .. 7.5).
    pub floor: f32,
    /// Window width as a share of the cell.
    pub win_w: f32,
}

/// Vertex alpha of a surface carrying `p` (see the codes above); a zero share is a plain
/// surface (alpha 1).
pub fn pattern_alpha(p: Pattern) -> f32 {
    let share = (p.share.clamp(0.0, 1.0) * 31.0).round() as u32;
    if share == 0 {
        return 1.0;
    }
    let class = match p.class {
        LightClass::Home => 0,
        LightClass::Work => 1,
        LightClass::Industry => 2,
    };
    let half = |m: f32, lo: u32| ((m * 2.0).round() as u32).clamp(lo, 15);
    let ww = ((p.win_w.clamp(0.0, 1.0) * 15.0).round() as u32).max(1);
    let code = share
        | class << 5
        | u32::from(p.band) << 7
        | half(p.bay, 2) << 8
        | half(p.floor, 4) << 12
        | ww << 16
        | PATTERN;
    2.0 + code as f32
}

/// Glow of a vertex alpha at `night` (0 day .. 1 night) and `late` ([`lateness`]), 0 for a
/// plain surface: a window's strength times how far it is on (the shader's formula), a
/// pattern's average over its cells (what the shader fades to where cells are below a pixel).
pub fn glow(alpha: f32, night: f32, late: f32) -> f32 {
    if alpha < 1.5 {
        return 0.0;
    }
    let code = (alpha - 2.0).round() as u32;
    if code & PATTERN != 0 {
        let share = (code & 31) as f32 / 31.0;
        let class = match (code >> 5) & 3 {
            0 => LightClass::Home,
            1 => LightClass::Work,
            _ => LightClass::Industry,
        };
        let band = (code >> 7) & 1 == 1;
        let win_w = ((code >> 16) & 15) as f32 / 15.0;
        let win_h = if band { 1.0 } else { PATTERN_WIN_H };
        return share * win_w * win_h * still_on(class, late) * night;
    }
    let q = code & 63;
    let s = ((code >> 8) & 31) as f32 / 31.0;
    let on = if q == ALL_NIGHT {
        1.0
    } else {
        // A quarter of an hour of game time.
        1.0 - smoothstep(q as f32 / 62.0 - 0.03, q as f32 / 62.0, late)
    };
    s * s * on * night
}

/// Share of the evening's lit windows of `class` still lit at `late`.
pub fn still_on(class: LightClass, late: f32) -> f32 {
    let (all_night, start, span) = switch_off(class);
    // A window is half off 0.015 before its switch-off time (see `glow`).
    all_night + (1.0 - all_night) * ((start + span - late - 0.015) / span).clamp(0.0, 1.0)
}

fn smoothstep(a: f32, b: f32, v: f32) -> f32 {
    let t = ((v - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Share of `windows` windows of a building lit at `late` (night fully dark): what
/// [`window_light`] gives, for tests and for the average glow of bands.
pub fn lit_share(class: Option<LightClass>, seed: u32, windows: u32, late: f32) -> f32 {
    let Some(class) = class else { return 0.0 };
    let lit = (0..windows)
        .filter(|&w| {
            window_light(class, seed, w)
                .is_some_and(|l| glow(glow_alpha(l.off, l.tint, 1.0), 1.0, late) > 0.5)
        })
        .count();
    lit as f32 / windows.max(1) as f32
}

#[cfg(test)]
mod tests {
    use super::*;
    use cp_core::Tick;
    use cp_core::time::TICKS_PER_DAY;

    fn at(hour: f32) -> f32 {
        lateness(Tick((hour / 24.0 * TICKS_PER_DAY as f32) as u64).game_time())
    }

    fn mean_share(class: LightClass, late: f32) -> f32 {
        (0..200)
            .map(|s| lit_share(Some(class), s * 7919, 400, late))
            .sum::<f32>()
            / 200.0
    }

    #[test]
    fn lateness_runs_from_the_evening_to_the_small_hours() {
        assert_eq!(at(15.0), 0.0);
        assert_eq!(at(19.0), 0.0);
        assert!((at(0.0) - 5.0 / 9.0).abs() < 0.01);
        assert_eq!(at(4.1), 1.0);
        assert_eq!(at(11.9), 1.0);
        assert!(at(21.0) < at(23.0) && at(23.0) < at(2.0));
    }

    #[test]
    fn the_pattern_is_deterministic_and_differs_between_buildings() {
        let a: Vec<_> = (0..64)
            .map(|w| window_light(LightClass::Home, 42, w))
            .collect();
        let b: Vec<_> = (0..64)
            .map(|w| window_light(LightClass::Home, 42, w))
            .collect();
        let c: Vec<_> = (0..64)
            .map(|w| window_light(LightClass::Home, 43, w))
            .collect();
        assert_eq!(a, b);
        assert_ne!(a, c);
        let tints: std::collections::BTreeSet<_> = (0..400)
            .filter_map(|w| window_light(LightClass::Home, 42, w))
            .map(|l| l.tint)
            .collect();
        assert_eq!(tints.len(), 4, "warm and cooler lamps");
    }

    #[test]
    fn homes_are_half_lit_in_the_evening_and_fewer_after_midnight() {
        let evening = at(20.0);
        let late = at(1.5);
        for s in 0..50 {
            let sh = lit_share(Some(LightClass::Home), s * 31, 600, evening);
            assert!((0.30..=0.65).contains(&sh), "seed {s}: {sh}");
        }
        let (e, l) = (
            mean_share(LightClass::Home, evening),
            mean_share(LightClass::Home, late),
        );
        assert!((0.40..=0.55).contains(&e), "{e}");
        assert!(l < 0.75 * e && l > 0.1, "evening {e}, 01:30 {l}");
    }

    #[test]
    fn offices_go_dark_late_and_industry_keeps_a_few_lights() {
        let (e, l) = (
            mean_share(LightClass::Work, at(20.0)),
            mean_share(LightClass::Work, at(2.0)),
        );
        assert!(e > 0.45, "offices lit in the evening: {e}");
        assert!(l < 0.1, "mostly dark late: {l}");
        let (ie, il) = (
            mean_share(LightClass::Industry, at(20.0)),
            mean_share(LightClass::Industry, at(3.0)),
        );
        assert!(ie > 0.05 && ie < 0.25, "{ie}");
        assert!(il > 0.05 && il <= ie, "{il}");
    }

    #[test]
    fn unpowered_and_inactive_buildings_are_dark() {
        let b = |state, flags| Building {
            def: cp_content::BuildingDefId(0),
            footprint: cp_building::Footprint::Free { w: 2, d: 2 },
            level: 1,
            state,
            ready_at: Tick(0),
            pos: bevy_math::Vec2::ZERO,
            rot: 0.0,
            access: None,
            flags: BuildingFlags(flags),
            level_progress: 0.0,
        };
        let lit = b(BuildingState::Active, 0);
        assert_eq!(
            light_class(&lit, ZoneType::ResLow, false),
            Some(LightClass::Home)
        );
        for dark in [
            b(BuildingState::Active, BuildingFlags::NO_POWER),
            b(
                BuildingState::Active,
                BuildingFlags::NO_POWER | BuildingFlags::NO_WATER,
            ),
            b(BuildingState::UnderConstruction, 0),
            b(BuildingState::Abandoned, 0),
            b(BuildingState::Burned, 0),
        ] {
            let class = light_class(&dark, ZoneType::Office, false);
            assert_eq!(class, None, "{:?} {:?}", dark.state, dark.flags);
            assert_eq!(lit_share(class, 9, 400, at(21.0)), 0.0);
        }
        assert_eq!(
            light_class(&lit, ZoneType::Industrial, true),
            Some(LightClass::Work),
            "a service lights like an office"
        );
        // No water alone does not put the lights out.
        let dry = b(BuildingState::Active, BuildingFlags::NO_WATER);
        assert_eq!(
            light_class(&dry, ZoneType::Office, false),
            Some(LightClass::Work)
        );
    }

    #[test]
    fn glow_codes_round_trip_and_follow_the_night() {
        assert_eq!(glow(1.0, 1.0, 0.5), 0.0, "a plain surface");
        assert_eq!(glow_alpha(Some(0.5), 1, 0.0), 1.0);
        let w = glow_alpha(Some(0.5), 3, 1.0);
        assert_eq!(glow(w, 0.0, 0.0), 0.0, "day");
        assert_eq!(glow(w, 1.0, 0.2), 1.0);
        assert!((glow(w, 0.5, 0.2) - 0.5).abs() < 1e-6, "dusk fades in");
        assert_eq!(glow(w, 1.0, 0.6), 0.0, "switched off");
        // Off over a few minutes, not at once.
        let mid = glow(w, 1.0, 0.485);
        assert!(mid > 0.05 && mid < 0.95, "{mid}");
        // Interpolation noise in the attribute does not change the code.
        assert_eq!(glow(w + 0.3, 1.0, 0.2), glow(w, 1.0, 0.2));
        let all = glow_alpha(None, 0, 1.0);
        assert_eq!(glow(all, 1.0, 1.0), 1.0, "until dawn");
        let faint = glow(glow_alpha(None, 1, 0.01), 1.0, 0.0);
        assert!(
            (faint - 0.01).abs() < 0.003,
            "fine steps near zero: {faint}"
        );
    }

    #[test]
    fn a_window_pattern_averages_what_single_windows_show() {
        let p = Pattern {
            class: LightClass::Home,
            share: 0.5,
            band: true,
            bay: 3.0,
            floor: 3.2,
            win_w: 0.6,
        };
        let a = pattern_alpha(p);
        assert!(a < (1 << 24) as f32, "exact in f32");
        assert_eq!(pattern_alpha(Pattern { share: 0.0, ..p }), 1.0);
        assert_eq!(glow(a, 0.0, 0.0), 0.0, "day");
        let evening = glow(a, 1.0, 0.0);
        assert!((evening - 0.5 * 0.6).abs() < 0.02, "{evening}");
        let wall = glow(pattern_alpha(Pattern { band: false, ..p }), 1.0, 0.0);
        assert!((wall - 0.5 * 0.6 * PATTERN_WIN_H).abs() < 0.02, "{wall}");
        // The average dims as single windows go dark.
        for class in [LightClass::Home, LightClass::Work, LightClass::Industry] {
            for late in [0.1, 0.4, 0.7, 0.95] {
                let windows = mean_share(class, late) / mean_share(class, 0.0);
                let average = still_on(class, late);
                assert!(
                    (windows - average).abs() < 0.04,
                    "{class:?} at {late}: windows {windows}, pattern {average}"
                );
            }
        }
        assert_eq!(still_on(LightClass::Work, 1.0), 0.08);
    }
}
