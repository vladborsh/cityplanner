//! Pure camera constraint math (no Bevy app, no window): tilt, bounds, height clearance,
//! terrain ray-cast and zoom-to-cursor. See `architecture/system-design.md` §13.2.
//!
//! World convention is `cp_terrain`'s: XZ ground plane, Y up, origin at the NW corner.
//! The camera orbits a `focus` point on the ground. `yaw = 0` puts the camera on the
//! +Z side of the focus looking towards -Z; `pitch` is the elevation angle of the camera
//! above the horizon (the "tilt").

use bevy_math::{Vec2, Vec3};
use cp_terrain::{MAP_SIZE, TILE_SIZE, TILES_PER_SIDE, Terrain, TileOwnership};

pub const MIN_TILT: f32 = 15.0 * std::f32::consts::PI / 180.0;
pub const MAX_TILT: f32 = 89.0 * std::f32::consts::PI / 180.0;
/// Minimum camera height above the terrain around it (m).
pub const MIN_CLEARANCE: f32 = 20.0;
/// Maximum camera height above sea datum (m).
pub const MAX_HEIGHT: f32 = 4000.0;
/// The focus may leave the owned-tile bounding box by this much (m).
pub const BOUNDS_MARGIN: f32 = 1000.0;
/// Smallest orbit distance (m).
pub const MIN_DISTANCE: f32 = 25.0;

/// Axis-aligned rectangle on the XZ plane.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bounds {
    pub min: Vec2,
    pub max: Vec2,
}

/// Bounding box of the owned tiles plus [`BOUNDS_MARGIN`]. With nothing owned, the
/// whole map (plus margin).
pub fn owned_bounds(tiles: &TileOwnership) -> Bounds {
    let (mut lo, mut hi) = (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN));
    for tz in 0..TILES_PER_SIDE {
        for tx in 0..TILES_PER_SIDE {
            if tiles.is_open(tx, tz) {
                lo = lo.min(Vec2::new(tx as f32, tz as f32) * TILE_SIZE);
                hi = hi.max(Vec2::new(tx as f32 + 1.0, tz as f32 + 1.0) * TILE_SIZE);
            }
        }
    }
    if lo.x > hi.x {
        (lo, hi) = (Vec2::ZERO, Vec2::splat(MAP_SIZE));
    }
    Bounds {
        min: lo - Vec2::splat(BOUNDS_MARGIN),
        max: hi + Vec2::splat(BOUNDS_MARGIN),
    }
}

pub fn clamp_to_bounds(p: Vec2, b: &Bounds) -> Vec2 {
    p.clamp(b.min, b.max)
}

pub fn clamp_tilt(pitch: f32) -> f32 {
    pitch.clamp(MIN_TILT, MAX_TILT)
}

/// Pitch after holding the tilt keys for `dt` seconds: `dir` is +1 for R (towards top-down),
/// -1 for F (towards the horizon), 0 for none; `speed` in rad/s. Clamped to the tilt limits.
pub fn key_tilt(pitch: f32, dir: i32, speed: f32, dt: f32) -> f32 {
    clamp_tilt(pitch + dir as f32 * speed * dt)
}

/// Highest terrain within ~60 m of `p` (sampled on two rings), so the camera does not
/// clip into a hill beside it. 0 without a terrain.
pub fn terrain_peak_near(terrain: Option<&Terrain>, p: Vec2) -> f32 {
    let Some(t) = terrain else { return 0.0 };
    let mut peak = t.height(p);
    for r in [25.0f32, 60.0] {
        for i in 0..8 {
            let a = i as f32 * std::f32::consts::FRAC_PI_4;
            peak = peak.max(t.height(p + Vec2::new(a.cos(), a.sin()) * r));
        }
    }
    peak
}

/// Clamps a camera position's height to `[terrain + 20 m, 4000 m]`. The lower bound wins.
pub fn clamp_camera_height(terrain: Option<&Terrain>, pos: Vec3) -> Vec3 {
    let floor = terrain_peak_near(terrain, Vec2::new(pos.x, pos.z)) + MIN_CLEARANCE;
    Vec3::new(pos.x, pos.y.min(MAX_HEIGHT).max(floor), pos.z)
}

/// Vector from the focus to the camera.
pub fn rig_offset(yaw: f32, pitch: f32, distance: f32) -> Vec3 {
    let h = distance * pitch.cos();
    Vec3::new(h * yaw.sin(), distance * pitch.sin(), h * yaw.cos())
}

/// Orbit state: where the camera looks and from how far/which angle.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rig {
    pub focus: Vec2,
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
}

impl Rig {
    /// The rig that sees `look` from `pos`.
    pub fn from_pose(pos: Vec3, look: Vec3) -> Self {
        let d = pos - look;
        let distance = d.length().max(MIN_DISTANCE);
        Self {
            focus: Vec2::new(look.x, look.z),
            yaw: d.x.atan2(d.z),
            pitch: (d.y / distance).clamp(-1.0, 1.0).asin(),
            distance,
        }
    }

    pub fn focus_point(&self, terrain: Option<&Terrain>) -> Vec3 {
        Vec3::new(
            self.focus.x,
            terrain.map_or(0.0, |t| t.height(self.focus)),
            self.focus.y,
        )
    }

    /// Applies tilt, bounds and distance limits (the height limit as a max distance).
    pub fn constrain(&mut self, terrain: Option<&Terrain>, bounds: &Bounds) {
        self.pitch = clamp_tilt(self.pitch);
        self.focus = clamp_to_bounds(self.focus, bounds);
        let focus_y = self.focus_point(terrain).y;
        let max_d = ((MAX_HEIGHT - focus_y) / self.pitch.sin()).max(MIN_DISTANCE);
        self.distance = self.distance.clamp(MIN_DISTANCE, max_d);
    }

    /// Camera position, with the 20 m / 4000 m height clamp applied.
    pub fn camera_position(&self, terrain: Option<&Terrain>) -> Vec3 {
        let p = self.focus_point(terrain) + rig_offset(self.yaw, self.pitch, self.distance);
        clamp_camera_height(terrain, p)
    }
}

/// First intersection of a ray with the terrain surface, or `None` if the origin is
/// already under it or nothing is hit within `max_dist`. `dir` must be normalised.
pub fn ray_terrain_hit(terrain: &Terrain, origin: Vec3, dir: Vec3, max_dist: f32) -> Option<Vec3> {
    let above = |t: f32| {
        let p = origin + dir * t;
        p.y - terrain.height(Vec2::new(p.x, p.z))
    };
    if above(0.0) < 0.0 {
        return None;
    }
    let mut t = 0.0f32;
    while t < max_dist {
        let clearance = above(t);
        let step = (clearance * 0.25).clamp(5.0, 200.0);
        let prev = t;
        t += step;
        if above(t) <= 0.0 {
            let (mut lo, mut hi) = (prev, t);
            for _ in 0..24 {
                let mid = 0.5 * (lo + hi);
                if above(mid) > 0.0 {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            return Some(origin + dir * (0.5 * (lo + hi)));
        }
    }
    None
}

/// Scales the view about `anchor` (the terrain point under the cursor) by `factor`
/// (< 1 zooms in): the anchor keeps its screen position. Returns the new focus point
/// and distance.
pub fn zoom_to_cursor(focus: Vec3, distance: f32, anchor: Vec3, factor: f32) -> (Vec3, f32) {
    (anchor + (focus - anchor) * factor, distance * factor)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cp_terrain::{GRID, MapMeta};

    /// Flat ground at `h` metres with a 300 m-high wall (nodes x = 100..110) on the map.
    fn terrain(h: f32, wall: bool) -> Terrain {
        let enc = |m: f32| (m / 1024.0 * 65535.0).round() as u16;
        let mut hs = vec![enc(h); GRID * GRID];
        if wall {
            for z in 0..GRID {
                for x in 100..110 {
                    hs[z * GRID + x] = enc(300.0);
                }
            }
        }
        let meta = MapMeta {
            name: "t".into(),
            sea_level: 0.0,
            start_tile: (2, 2),
            external_connections: vec![],
        };
        Terrain::from_samples(meta, &hs, &vec![0; GRID * GRID]).unwrap()
    }

    #[test]
    fn r_raises_and_f_lowers_the_tilt_within_limits() {
        let deg = std::f32::consts::PI / 180.0;
        let start = 45.0 * deg;
        // 1 s at 1 rad/s is 57.3 deg: R from 45 deg would reach 102 deg, so it stops at 89.
        assert_eq!(key_tilt(start, 1, 1.0, 1.0), MAX_TILT);
        // F mirrors it: 45 - 57.3 < 15.
        assert_eq!(key_tilt(start, -1, 1.0, 1.0), MIN_TILT);
        // Below the limits it is plain integration: 0.2 s of R = +11.46 deg.
        assert!((key_tilt(start, 1, 1.0, 0.2) - (start + 0.2)).abs() < 1e-6);
        assert!((key_tilt(start, -1, 1.0, 0.2) - (start - 0.2)).abs() < 1e-6);
        // No key: unchanged. 1 s in 60 frames reaches the same clamp.
        assert_eq!(key_tilt(start, 0, 1.0, 1.0), start);
        let mut p = start;
        for _ in 0..60 {
            p = key_tilt(p, 1, 1.0, 1.0 / 60.0);
        }
        assert_eq!(p, MAX_TILT);
    }

    #[test]
    fn tilt_is_clamped_to_15_89_degrees() {
        assert_eq!(clamp_tilt(0.0), MIN_TILT);
        assert_eq!(clamp_tilt(-3.0), MIN_TILT);
        assert_eq!(clamp_tilt(10.0), MAX_TILT);
        assert_eq!(clamp_tilt(0.8), 0.8);
        assert!((MIN_TILT.to_degrees() - 15.0).abs() < 1e-4);
        assert!((MAX_TILT.to_degrees() - 89.0).abs() < 1e-4);
    }

    #[test]
    fn bounds_are_owned_tiles_plus_one_km() {
        let b = owned_bounds(&TileOwnership::m1_around((2, 2)));
        assert_eq!(b.min, Vec2::splat(TILE_SIZE - 1000.0));
        assert_eq!(b.max, Vec2::splat(4.0 * TILE_SIZE + 1000.0));
        let one = owned_bounds(&TileOwnership::only((0, 4)));
        assert_eq!(one.min, Vec2::new(-1000.0, 4.0 * TILE_SIZE - 1000.0));
        assert_eq!(one.max, Vec2::new(TILE_SIZE + 1000.0, MAP_SIZE + 1000.0));
        let none = owned_bounds(&TileOwnership::none());
        assert_eq!(none.max, Vec2::splat(MAP_SIZE + 1000.0));
    }

    #[test]
    fn focus_is_clamped_into_bounds() {
        let b = owned_bounds(&TileOwnership::m1_around((2, 2)));
        assert_eq!(
            clamp_to_bounds(Vec2::new(-9e3, 5000.0), &b),
            Vec2::new(b.min.x, 5000.0)
        );
        assert_eq!(clamp_to_bounds(Vec2::splat(1e9), &b), b.max);
        let inside = Vec2::new(5000.0, 6000.0);
        assert_eq!(clamp_to_bounds(inside, &b), inside);
    }

    #[test]
    fn camera_never_below_terrain_plus_20_or_above_4000() {
        let t = terrain(50.0, false);
        let low = clamp_camera_height(Some(&t), Vec3::new(5000.0, 0.0, 5000.0));
        assert!((low.y - 70.0).abs() < 0.05);
        let high = clamp_camera_height(Some(&t), Vec3::new(5000.0, 9e5, 5000.0));
        assert_eq!(high.y, MAX_HEIGHT);
        assert_eq!(
            clamp_camera_height(None, Vec3::new(0.0, -5.0, 0.0)).y,
            MIN_CLEARANCE
        );
    }

    #[test]
    fn height_clamp_looks_at_neighbouring_hills() {
        let t = terrain(10.0, true);
        // Wall nodes 100..110 span x = 1000..1090 m. A camera 30 m beside it still
        // clears the 300 m ridge.
        let p = clamp_camera_height(Some(&t), Vec3::new(1000.0 - 30.0, 0.0, 5000.0));
        assert!(p.y >= 320.0 - 0.05, "{}", p.y);
        let far = clamp_camera_height(Some(&t), Vec3::new(3000.0, 0.0, 5000.0));
        assert!((far.y - 30.0).abs() < 0.05);
    }

    #[test]
    fn rig_pose_roundtrip() {
        let pos = Vec3::new(5000.0, 800.0, 7000.0);
        let look = Vec3::new(5000.0, 0.0, 5000.0);
        let r = Rig::from_pose(pos, look);
        let back = r.focus_point(None) + rig_offset(r.yaw, r.pitch, r.distance);
        assert!((back - pos).length() < 0.05);
        assert!(r.yaw.abs() < 1e-5);
    }

    #[test]
    fn constrain_applies_all_limits() {
        let t = terrain(100.0, false);
        let b = owned_bounds(&TileOwnership::m1_around((2, 2)));
        let mut r = Rig {
            focus: Vec2::splat(-1e6),
            yaw: 0.0,
            pitch: 0.0,
            distance: 1e9,
        };
        r.constrain(Some(&t), &b);
        assert_eq!(r.focus, b.min);
        assert_eq!(r.pitch, MIN_TILT);
        let cam = r.camera_position(Some(&t));
        assert!(cam.y <= MAX_HEIGHT + 0.01 && cam.y >= 120.0);
        r.distance = 0.0;
        r.pitch = 10.0;
        r.constrain(Some(&t), &b);
        assert_eq!((r.pitch, r.distance), (MAX_TILT, MIN_DISTANCE));
        assert!(r.camera_position(Some(&t)).y >= 120.0 - 0.05);
    }

    #[test]
    fn ray_hits_flat_ground_and_hill() {
        let t = terrain(50.0, true);
        let dir = Vec3::new(0.0, -1.0, 1.0).normalize();
        let hit = ray_terrain_hit(&t, Vec3::new(5000.0, 450.0, 4000.0), dir, 10_000.0).unwrap();
        assert!((hit.y - 50.0).abs() < 0.1);
        assert!((hit.z - 4400.0).abs() < 0.5);
        // Toward the wall: stops on its east flank (x 1090..1100), well above the plain.
        let dir = Vec3::new(-1.0, -0.5, 0.0).normalize();
        let hit = ray_terrain_hit(&t, Vec3::new(1500.0, 400.0, 3000.0), dir, 10_000.0).unwrap();
        assert!(hit.x > 1090.0 && hit.x < 1100.0 && hit.y > 60.0, "{hit:?}");
        assert!((hit.y - t.height(Vec2::new(hit.x, hit.z))).abs() < 0.1);
    }

    #[test]
    fn ray_misses_when_pointing_up_or_starting_underground() {
        let t = terrain(50.0, false);
        assert!(ray_terrain_hit(&t, Vec3::new(5000.0, 450.0, 4000.0), Vec3::Y, 1e4).is_none());
        assert!(ray_terrain_hit(&t, Vec3::new(5000.0, 10.0, 4000.0), -Vec3::Y, 1e4).is_none());
    }

    #[test]
    fn zoom_keeps_the_cursor_point_fixed_on_screen() {
        let t = terrain(50.0, false);
        let rig = Rig {
            focus: Vec2::new(5000.0, 5000.0),
            yaw: 0.7,
            pitch: 0.9,
            distance: 600.0,
        };
        let cam = rig.camera_position(Some(&t));
        let anchor = Vec3::new(5200.0, 50.0, 4800.0);
        let (focus, d) = zoom_to_cursor(rig.focus_point(Some(&t)), rig.distance, anchor, 0.5);
        assert_eq!(d, 300.0);
        let zoomed = Rig {
            focus: Vec2::new(focus.x, focus.z),
            distance: d,
            ..rig
        };
        let cam2 = zoomed.camera_position(Some(&t));
        // Similarity about the anchor: the camera moves half way to it.
        assert!((cam2 - (anchor + (cam - anchor) * 0.5)).length() < 0.05);
        // Zooming out is the inverse.
        let (f3, d3) = zoom_to_cursor(focus, d, anchor, 2.0);
        assert!((f3 - rig.focus_point(Some(&t))).length() < 0.05 && (d3 - 600.0).abs() < 1e-3);
    }
}
