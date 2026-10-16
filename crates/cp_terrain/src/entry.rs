//! The entry highway of a map (`system-design.md` §5.1): its geometry helpers and the check
//! that a map on disk has a usable one.

use bevy_math::Vec2;

use crate::{EntryRail, EntryRoad, MAP_SIZE, TILE_SIZE, Terrain};

/// Distance of the junction from the start tile's edge, at least, metres.
pub const MIN_EDGE_MARGIN: f32 = 64.0;
/// Smallest angle between two legs of the centre line, degrees.
pub const MIN_LEG_ANGLE_DEG: f32 = 150.0;
/// Steepest terrain along the entry highway.
pub const MAX_SLOPE: f32 = 0.06;
/// Offset of each of the two one-way highways from the centre line, metres.
pub const LANE_OFFSET: f32 = 16.0;
/// The two highways merge into the junction over this much of the centre line's end, metres.
pub const MERGE_LENGTH: f32 = 40.0;
/// Longest leg of the two highways, metres.
pub const MAX_LEG: f32 = 80.0;
/// Spacing of the samples along the line, metres.
const STEP: f32 = 4.0;

impl EntryRoad {
    /// The centre line as points.
    pub fn points(&self) -> Vec<Vec2> {
        self.path.iter().map(|&(x, z)| Vec2::new(x, z)).collect()
    }

    /// The junction `J`: the last point of the centre line.
    pub fn junction(&self) -> Vec2 {
        let (x, z) = *self.path.last().expect("path has points");
        Vec2::new(x, z)
    }

    /// Unit direction of the last leg (into the city).
    pub fn direction(&self) -> Vec2 {
        let n = self.path.len();
        let (a, b) = (self.path[n - 2], self.path[n - 1]);
        (Vec2::new(b.0 - a.0, b.1 - a.1)).normalize_or_zero()
    }

    /// The dead end of the stub.
    pub fn stub_end(&self) -> Vec2 {
        self.junction() + self.direction() * self.stub_m
    }

    /// The two one-way highways as polylines in travel direction: inbound from the map edge to
    /// the junction (the right-hand carriageway), outbound back to the edge. Both end at
    /// [`EntryRoad::junction`] and run [`LANE_OFFSET`] off the centre line until they merge over
    /// its last [`MERGE_LENGTH`] metres. Legs are at most [`MAX_LEG`] metres.
    pub fn carriageways(&self) -> (Vec<Vec2>, Vec<Vec2>) {
        let left = |t: Vec2| Vec2::new(t.y, -t.x);
        let run = (self.length() - MERGE_LENGTH).max(1.0);
        let n = (run / MAX_LEG).ceil().max(1.0) as usize;
        let side = |sign: f32| {
            let mut pts: Vec<Vec2> = (0..=n)
                .map(|i| {
                    let (p, t) = self.at(run * i as f32 / n as f32);
                    p + left(t) * (sign * LANE_OFFSET)
                })
                .collect();
            pts.push(self.junction());
            // The end at the map edge lies on it, whatever the angle of the first leg.
            let (c, p) = (self.path[0], &mut pts[0]);
            if c.0.abs() < 0.01 || (c.0 - MAP_SIZE).abs() < 0.01 {
                p.x = c.0;
            }
            if c.1.abs() < 0.01 || (c.1 - MAP_SIZE).abs() < 0.01 {
                p.y = c.1;
            }
            pts
        };
        let inbound = side(-1.0);
        let mut outbound = side(1.0);
        outbound.reverse();
        (inbound, outbound)
    }

    /// Arc length of the centre line.
    pub fn length(&self) -> f32 {
        self.points().windows(2).map(|w| w[0].distance(w[1])).sum()
    }

    /// Point and unit tangent at arc length `s` along the centre line (clamped).
    pub fn at(&self, s: f32) -> (Vec2, Vec2) {
        let pts = self.points();
        let mut left = s.max(0.0);
        for w in pts.windows(2) {
            let l = w[0].distance(w[1]);
            if left <= l || std::ptr::eq(w, &pts[pts.len() - 2..]) {
                let t = (w[1] - w[0]).normalize_or_zero();
                return (w[0] + t * left.min(l), t);
            }
            left -= l;
        }
        (self.junction(), self.direction())
    }
}

fn in_tile(p: Vec2, tile: (u8, u8), margin: f32) -> bool {
    let (x0, z0) = (tile.0 as f32 * TILE_SIZE, tile.1 as f32 * TILE_SIZE);
    p.x >= x0 + margin
        && p.x <= x0 + TILE_SIZE - margin
        && p.y >= z0 + margin
        && p.y <= z0 + TILE_SIZE - margin
}

impl Terrain {
    /// Checks the map's `entry_road` (the error says what is wrong): the connection point,
    /// the junction and the stub inside the start tile, smooth legs, no water, gentle ground.
    pub fn validate_entry(&self) -> Result<(), String> {
        let meta = self.meta();
        let e = meta
            .entry_road
            .as_ref()
            .ok_or_else(|| "entry_road is missing".to_string())?;
        let err = |m: String| Err(format!("entry_road: {m}"));
        let Some(&(cx, cz)) = meta.external_connections.get(e.connection) else {
            return err(format!(
                "connection {} is not one of the {} external connections",
                e.connection,
                meta.external_connections.len()
            ));
        };
        if e.path.len() < 2 || e.path.iter().any(|p| !p.0.is_finite() || !p.1.is_finite()) {
            return err("path needs at least two finite points".into());
        }
        let first = Vec2::new(e.path[0].0, e.path[0].1);
        if first.distance(Vec2::new(cx, cz)) > 0.5 {
            return err(format!(
                "path starts at ({}, {}), not at external connection {} ({cx}, {cz})",
                e.path[0].0, e.path[0].1, e.connection
            ));
        }
        let on_edge = |v: f32| v.abs() < 0.01 || (v - MAP_SIZE).abs() < 0.01;
        if !on_edge(cx) && !on_edge(cz) {
            return err(format!(
                "external connection ({cx}, {cz}) is not on the map edge"
            ));
        }
        let pts = e.points();
        if pts.windows(2).any(|w| w[0].distance(w[1]) < 8.0) {
            return err("every leg of the path must be at least 8 m long".into());
        }
        if e.length() < 100.0 {
            return err("path is shorter than 100 m".into());
        }
        for (i, w) in pts.windows(3).enumerate() {
            let (a, b) = (w[0] - w[1], w[2] - w[1]);
            let angle = a.angle_to(b).abs().to_degrees();
            if angle < MIN_LEG_ANGLE_DEG {
                return err(format!(
                    "angle {angle:.0}° at path point {} is sharper than {MIN_LEG_ANGLE_DEG:.0}°",
                    i + 1
                ));
            }
        }
        let tile = meta.start_tile;
        let j = e.junction();
        if !in_tile(j, tile, MIN_EDGE_MARGIN) {
            return err(format!(
                "the last point ({}, {}) must lie inside start tile {tile:?}, at least {MIN_EDGE_MARGIN:.0} m from its edge",
                j.x, j.y
            ));
        }
        if e.stub_m.is_nan() || e.stub_m <= 0.0 || !in_tile(e.stub_end(), tile, 0.0) {
            return err(format!(
                "the stub ({} m from the last point) must end inside start tile {tile:?}",
                e.stub_m
            ));
        }
        // Dry within 20 m of the centre line (both highways and their width), and gentle along
        // the centre line, the stub and both highways as they will be built.
        let mut legs = pts.clone();
        legs.push(e.stub_end());
        let (inbound, outbound) = e.carriageways();
        let mut lines: Vec<Vec<Vec2>> = vec![legs.clone(), inbound, outbound];
        for w in legs.windows(2) {
            let t = (w[1] - w[0]).normalize_or_zero();
            let side = Vec2::new(t.y, -t.x);
            for off in [-20.0, 20.0] {
                lines.push(vec![w[0] + side * off, w[1] + side * off]);
            }
        }
        for (li, line) in lines.iter().enumerate() {
            let slope_checked = li < 3;
            for w in line.windows(2) {
                let (a, b) = (w[0], w[1]);
                let len = a.distance(b);
                let n = (len / STEP).ceil().max(1.0) as usize;
                let mut prev: Option<f32> = None;
                for k in 0..=n {
                    let q = a.lerp(b, k as f32 / n as f32);
                    if self.is_water(q) {
                        return err(format!(
                            "water at ({:.0}, {:.0}); pick a connection that does not cross it",
                            q.x, q.y
                        ));
                    }
                    if !slope_checked {
                        continue;
                    }
                    let h = self.height(q);
                    if let Some(ph) = prev {
                        let slope = (h - ph).abs() / (len / n as f32);
                        if slope > MAX_SLOPE {
                            return err(format!(
                                "slope {:.0}% at ({:.0}, {:.0}) is over {:.0}%",
                                slope * 100.0,
                                q.x,
                                q.y,
                                MAX_SLOPE * 100.0
                            ));
                        }
                    }
                    prev = Some(h);
                }
            }
        }
        Ok(())
    }
}

/// Steepest terrain along the entry railway (the `max_slope` of `rail_2t`).
pub const RAIL_MAX_SLOPE: f32 = 0.04;

impl EntryRail {
    pub fn points(&self) -> Vec<Vec2> {
        self.path.iter().map(|&(x, z)| Vec2::new(x, z)).collect()
    }

    /// The dead end inside the start tile: the last point.
    pub fn dead_end(&self) -> Vec2 {
        let (x, z) = *self.path.last().expect("path has points");
        Vec2::new(x, z)
    }

    pub fn length(&self) -> f32 {
        self.points().windows(2).map(|w| w[0].distance(w[1])).sum()
    }
}

impl Terrain {
    /// Checks the map's `entry_rail`, if it has one: starts on the map edge, ends inside the
    /// start tile, gentle bends, dry and gentle ground. `Ok` without one.
    pub fn validate_entry_rail(&self) -> Result<(), String> {
        let meta = self.meta();
        let Some(e) = meta.entry_rail.as_ref() else {
            return Ok(());
        };
        let err = |m: String| Err(format!("entry_rail: {m}"));
        if e.path.len() < 2 || e.path.iter().any(|p| !p.0.is_finite() || !p.1.is_finite()) {
            return err("path needs at least two finite points".into());
        }
        let on_edge = |v: f32| v.abs() < 0.01 || (v - MAP_SIZE).abs() < 0.01;
        let (sx, sz) = e.path[0];
        if !on_edge(sx) && !on_edge(sz) {
            return err(format!(
                "the path starts at ({sx}, {sz}), not on the map edge"
            ));
        }
        let pts = e.points();
        if pts.windows(2).any(|w| w[0].distance(w[1]) < 8.0) {
            return err("every leg of the path must be at least 8 m long".into());
        }
        for (i, w) in pts.windows(3).enumerate() {
            let (a, b) = (w[0] - w[1], w[2] - w[1]);
            let angle = a.angle_to(b).abs().to_degrees();
            if angle < MIN_LEG_ANGLE_DEG {
                return err(format!(
                    "angle {angle:.0}° at path point {} is sharper than {MIN_LEG_ANGLE_DEG:.0}°",
                    i + 1
                ));
            }
        }
        let tile = meta.start_tile;
        let end = e.dead_end();
        if !in_tile(end, tile, MIN_EDGE_MARGIN) {
            return err(format!(
                "the dead end ({}, {}) must lie inside start tile {tile:?}, at least {MIN_EDGE_MARGIN:.0} m from its edge",
                end.x, end.y
            ));
        }
        // Dry within 12 m of the centre line, gentle along it.
        for w in pts.windows(2) {
            let (a, b) = (w[0], w[1]);
            let len = a.distance(b);
            let n = (len / STEP).ceil().max(1.0) as usize;
            let t = (b - a).normalize_or_zero();
            let side = Vec2::new(t.y, -t.x);
            let mut prev: Option<f32> = None;
            for k in 0..=n {
                let q = a.lerp(b, k as f32 / n as f32);
                for off in [-12.0, 0.0, 12.0] {
                    if self.is_water(q + side * off) {
                        return err(format!(
                            "water at ({:.0}, {:.0}); pick a path that does not cross it",
                            q.x, q.y
                        ));
                    }
                }
                let h = self.height(q);
                if let Some(ph) = prev {
                    let slope = (h - ph).abs() / (len / n as f32);
                    if slope > RAIL_MAX_SLOPE {
                        return err(format!(
                            "slope {:.0}% at ({:.0}, {:.0}) is over {:.0}%",
                            slope * 100.0,
                            q.x,
                            q.y,
                            RAIL_MAX_SLOPE * 100.0
                        ));
                    }
                }
                prev = Some(h);
            }
        }
        Ok(())
    }
}
