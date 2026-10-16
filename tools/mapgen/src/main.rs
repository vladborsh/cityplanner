//! Deterministic map generator: `cargo run -p mapgen [-- <out-dir>]`.
//! Writes `flatlands` and `river_valley` (map.ron, height.png, water.png) under
//! `assets/maps/` by default. Fixed seeds; output is committed, tests never regenerate it.

use std::fs;
use std::io::BufWriter;
use std::path::{Path, PathBuf};

use cp_terrain::{EntryRail, EntryRoad, GRID, MAX_HEIGHT, MapMeta, SPACING};

const SEED: u32 = 0xC17_91A7;

fn hash(x: i32, z: i32, seed: u32) -> f32 {
    let mut h = (x as u32).wrapping_mul(0x9E37_79B1)
        ^ (z as u32).wrapping_mul(0x85EB_CA77)
        ^ seed.wrapping_mul(0xC2B2_AE3D);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297A_2D39);
    h ^= h >> 15;
    (h & 0xFF_FFFF) as f32 / 0xFF_FFFF as f32
}

/// Smooth value noise in `0..1`; `scale` is the feature size in metres.
fn noise(x: f32, z: f32, scale: f32, seed: u32) -> f32 {
    let (gx, gz) = (x / scale, z / scale);
    let (ix, iz) = (gx.floor(), gz.floor());
    let (fx, fz) = (gx - ix, gz - iz);
    let s = |t: f32| t * t * (3.0 - 2.0 * t);
    let (ix, iz) = (ix as i32, iz as i32);
    let a = hash(ix, iz, seed);
    let b = hash(ix + 1, iz, seed);
    let c = hash(ix, iz + 1, seed);
    let d = hash(ix + 1, iz + 1, seed);
    let top = a + (b - a) * s(fx);
    let bot = c + (d - c) * s(fx);
    top + (bot - top) * s(fz)
}

fn fbm(x: f32, z: f32, scale: f32, seed: u32) -> f32 {
    let mut sum = 0.0;
    let mut amp = 0.5;
    let mut total = 0.0;
    for o in 0..4 {
        sum += amp * noise(x, z, scale / (1 << o) as f32, seed + o);
        total += amp;
        amp *= 0.5;
    }
    sum / total
}

fn smoothstep(a: f32, b: f32, v: f32) -> f32 {
    let t = ((v - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn to_u16(m: f32) -> u16 {
    (m / MAX_HEIGHT * 65535.0).round().clamp(0.0, 65535.0) as u16
}

/// The entry highway: the two one-way highways and the 6-lane stub the player continues.
fn entry_road(connection: usize, path: Vec<(f32, f32)>) -> EntryRoad {
    EntryRoad {
        connection,
        path,
        highway: "highway_1w_2l".into(),
        stub: "large_6l".into(),
        stub_m: 48.0,
    }
}

/// The entry railway: a straight line from the map edge to a dead end in the start tile, on
/// ground that was checked to be dry and gentle (it must not touch the heights: that
/// would change the map hash).
fn entry_rail(path: Vec<(f32, f32)>) -> EntryRail {
    EntryRail {
        path,
        road: "rail_2t".into(),
    }
}

struct Map {
    meta: MapMeta,
    height: Vec<u16>,
    water: Vec<u16>,
}

/// Plains rising gently from a sea along the west edge (sea level 20 m).
fn flatlands() -> Map {
    let mut height = vec![0u16; GRID * GRID];
    for iz in 0..GRID {
        for ix in 0..GRID {
            let (x, z) = (ix as f32 * SPACING, iz as f32 * SPACING);
            // Coast line wiggles around x = 1800 m; sea floor falls off to the west.
            let coast = 1800.0 + 500.0 * (fbm(0.0, z, 1500.0, SEED) - 0.5);
            let inland = smoothstep(coast - 600.0, coast + 300.0, x);
            let base = 8.0 + 22.0 * inland + 6.0 * (fbm(x, z, 2500.0, SEED + 10) - 0.5) * inland;
            let m = base + 1.5 * (fbm(x, z, 300.0, SEED + 20) - 0.5) * inland;
            height[iz * GRID + ix] = to_u16(m);
        }
    }
    Map {
        meta: MapMeta {
            name: "flatlands".into(),
            sea_level: 20.0,
            start_tile: (2, 2),
            external_connections: vec![(5120.0, 0.0), (10240.0, 5120.0), (5120.0, 10240.0)],
            entry_road: Some(entry_road(0, vec![(5120.0, 0.0), (5120.0, 4300.0)])),
            entry_rail: Some(entry_rail(vec![(10240.0, 5500.0), (6000.0, 5500.0)])),
        },
        height,
        water: vec![0; GRID * GRID],
    }
}

/// The entry highway's centre line of `river_valley`: from the north edge down the dry west
/// bank of the river to the start tile, a long gentle curve (every turn is a few degrees).
fn valley_entry_path() -> Vec<(f32, f32)> {
    let x_at = |z: f32| 5120.0 - 20.0 * z / 4400.0 + 40.0 * (z / 1100.0).sin();
    let mut path: Vec<(f32, f32)> = (0..55)
        .map(|i| {
            let z = i as f32 * 80.0;
            (x_at(z), z)
        })
        .collect();
    path[0] = (5120.0, 0.0);
    path.push((x_at(4400.0), 4400.0));
    path
}

/// How much of a point is a meadow, `0..=1` (T-180): wide flat clearings. Rules every new map
/// has to keep (the tests of this crate check them): the start tile is a meadow, the entry
/// highway runs through one, and meadows cover about half of the rest as clearings of
/// 600-1500 m between the hills.
fn meadow(x: f32, z: f32, entry: &[(f32, f32)]) -> f32 {
    let clearings = smoothstep(0.30, 0.62, noise(x, z, 3000.0, SEED + 60));
    let start = 1.0
        - smoothstep(
            1450.0,
            2200.0,
            ((x - 5120.0).powi(2) + (z - 5120.0).powi(2)).sqrt(),
        );
    let road = entry
        .iter()
        .map(|&(px, pz)| ((x - px).powi(2) + (z - pz).powi(2)).sqrt())
        .fold(f32::MAX, f32::min);
    let corridor = 1.0 - smoothstep(250.0, 1150.0, road);
    clearings.max(start).max(corridor)
}

/// Hills and a meandering river running north to south, with wide meadows between the hills.
fn river_valley() -> Map {
    let entry = valley_entry_path();
    let mut height = vec![0u16; GRID * GRID];
    let mut water = vec![0u16; GRID * GRID];
    for iz in 0..GRID {
        for ix in 0..GRID {
            let (x, z) = (ix as f32 * SPACING, iz as f32 * SPACING);
            let cx = 5120.0 + 1400.0 * (z / 1900.0).sin() + 500.0 * (z / 700.0 + 1.3).sin();
            let d = (x - cx).abs();
            let floor = 80.0 - z * 0.004;
            let hills = 90.0 * fbm(x, z, 2200.0, SEED + 30) + 10.0 * fbm(x, z, 500.0, SEED + 40);
            // Higher ground towards the edges of the map.
            let edge = (((x - 5120.0).powi(2) + (z - 5120.0).powi(2)).sqrt() - 3500.0) / 1500.0;
            let rim = 60.0 * smoothstep(0.0, 1.0, edge) * fbm(x, z, 1500.0, SEED + 90);
            let carve = smoothstep(30.0, 220.0, d);
            let span = smoothstep(120.0, 1600.0, d);
            let m = meadow(x, z, &entry);
            // A meadow is a few metres of slow swell (well under 1 %) and a little grain.
            let gentle = 3.0 * fbm(x, z, 1500.0, SEED + 70) + 0.4 * fbm(x, z, 120.0, SEED + 80);
            let rough = 6.0 * fbm(x, z, 600.0, SEED + 50) + (hills + rim) * span;
            let ground = floor - 4.0 + (4.0 + gentle + rough * (1.0 - m)) * carve;
            let surface = floor - 1.0;
            height[iz * GRID + ix] = to_u16(ground);
            if ground < surface && d < 120.0 {
                water[iz * GRID + ix] = to_u16(surface);
            }
        }
    }
    Map {
        meta: MapMeta {
            name: "river_valley".into(),
            sea_level: 5.0,
            start_tile: (2, 2),
            external_connections: vec![(5120.0, 0.0), (5120.0, 10240.0), (0.0, 5120.0)],
            entry_road: Some(entry_road(0, entry)),
            entry_rail: Some(entry_rail(vec![(0.0, 4250.0), (4250.0, 4250.0)])),
        },
        height,
        water,
    }
}

fn png16_bytes(data: &[u16]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(BufWriter::new(&mut out), GRID as u32, GRID as u32);
    enc.set_color(png::ColorType::Grayscale);
    enc.set_depth(png::BitDepth::Sixteen);
    let mut w = enc.write_header().unwrap();
    let bytes: Vec<u8> = data.iter().flat_map(|v| v.to_be_bytes()).collect();
    w.write_image_data(&bytes).unwrap();
    w.finish().unwrap();
    out
}

/// Steepness of every sample: the largest height step to a neighbour over the spacing.
fn slopes(height: &[u16]) -> Vec<f32> {
    let h = |ix: usize, iz: usize| f32::from(height[iz * GRID + ix]) / 65535.0 * MAX_HEIGHT;
    let mut out = vec![0.0; GRID * GRID];
    for iz in 0..GRID {
        for ix in 0..GRID {
            let mut worst = 0.0f32;
            for (dx, dz) in [(1i32, 0i32), (-1, 0), (0, 1), (0, -1)] {
                let (nx, nz) = (ix as i32 + dx, iz as i32 + dz);
                if (0..GRID as i32).contains(&nx) && (0..GRID as i32).contains(&nz) {
                    worst = worst.max((h(nx as usize, nz as usize) - h(ix, iz)).abs() / SPACING);
                }
            }
            out[iz * GRID + ix] = worst;
        }
    }
    out
}

/// Slope map for a look at the terrain: 8-bit grey, white is 40 % and steeper.
fn write_slope_map(path: &Path, height: &[u16]) {
    let mut enc = png::Encoder::new(
        BufWriter::new(fs::File::create(path).unwrap()),
        GRID as u32,
        GRID as u32,
    );
    enc.set_color(png::ColorType::Grayscale);
    enc.set_depth(png::BitDepth::Eight);
    let mut w = enc.write_header().unwrap();
    let bytes: Vec<u8> = slopes(height)
        .iter()
        .map(|s| (s / 0.4 * 255.0).clamp(0.0, 255.0) as u8)
        .collect();
    w.write_image_data(&bytes).unwrap();
}

fn write_png16(path: &Path, data: &[u16]) {
    fs::write(path, png16_bytes(data)).unwrap();
}

fn write_map(root: &Path, map: &Map) {
    let dir = root.join(&map.meta.name);
    fs::create_dir_all(&dir).unwrap();
    let cfg = ron::ser::PrettyConfig::default();
    fs::write(
        dir.join("map.ron"),
        ron::ser::to_string_pretty(&map.meta, cfg).unwrap(),
    )
    .unwrap();
    write_png16(&dir.join("height.png"), &map.height);
    write_png16(&dir.join("water.png"), &map.water);
    println!("wrote {}", dir.display());
}

fn main() {
    let root = std::env::args()
        .nth(1)
        .filter(|a| a != "--slope-map")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/maps"));
    // `--slope-map <png>`: also writes the steepness of river_valley (a look, not committed).
    if let Some(out) = std::env::args()
        .nth(2)
        .filter(|_| std::env::args().nth(1).as_deref() == Some("--slope-map"))
    {
        write_slope_map(Path::new(&out), &river_valley().height);
        return;
    }
    for map in [flatlands(), river_valley()] {
        write_map(&root, &map);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy_math::Vec2;
    use cp_terrain::{TILE_SIZE, Terrain};

    /// Slope up to which ground is "flat" for building (T-180).
    const FLAT_SLOPE: f32 = 0.04;
    /// Smallest share of the start tile that is flat, and of its 3x3-lot windows (24 m) that
    /// are flat everywhere. Every map the generator writes has to keep both.
    const MIN_FLAT_SHARE: f32 = 0.70;
    const MIN_OPEN_3X3_SHARE: f32 = 0.50;

    fn terrain(map: &Map) -> Terrain {
        Terrain::from_samples(map.meta.clone(), &map.height, &map.water).unwrap()
    }

    /// `(flat share of points, share of 24 m windows flat throughout)` of the start tile.
    fn flat_shares(t: &Terrain) -> (f32, f32) {
        let (tx, tz) = t.meta().start_tile;
        let (x0, z0) = (f32::from(tx) * TILE_SIZE, f32::from(tz) * TILE_SIZE);
        let flat = |x: f32, z: f32| {
            let h = t.height(Vec2::new(x, z));
            [(8.0, 0.0), (0.0, 8.0)]
                .iter()
                .all(|(dx, dz)| (t.height(Vec2::new(x + dx, z + dz)) - h).abs() / 8.0 <= FLAT_SLOPE)
                && !t.is_water(Vec2::new(x, z))
        };
        let n = (TILE_SIZE / 16.0) as usize;
        let mut yes = 0;
        for i in 0..n {
            for j in 0..n {
                yes += usize::from(flat(x0 + i as f32 * 16.0, z0 + j as f32 * 16.0));
            }
        }
        let w = (TILE_SIZE / 24.0) as usize - 1;
        let mut open = 0;
        for i in 0..w {
            for j in 0..w {
                let (wx, wz) = (x0 + i as f32 * 24.0, z0 + j as f32 * 24.0);
                let all = (0..=3)
                    .all(|a| (0..=3).all(|b| flat(wx + a as f32 * 8.0, wz + b as f32 * 8.0)));
                open += usize::from(all);
            }
        }
        (yes as f32 / (n * n) as f32, open as f32 / (w * w) as f32)
    }

    #[test]
    fn every_map_has_a_flat_start_tile_and_a_valid_entry_road() {
        for map in [flatlands(), river_valley()] {
            let t = terrain(&map);
            let (flat, open) = flat_shares(&t);
            println!("{}: flat {flat:.2}, open 3x3 {open:.2}", map.meta.name);
            assert!(
                flat >= MIN_FLAT_SHARE,
                "{}: flat share {flat}",
                map.meta.name
            );
            assert!(
                open >= MIN_OPEN_3X3_SHARE,
                "{}: open 3x3 share {open}",
                map.meta.name
            );
            t.validate_entry()
                .unwrap_or_else(|e| panic!("{}: {e}", map.meta.name));
        }
    }

    /// The committed files are what the generator writes (nothing edits them by hand).
    #[test]
    fn the_generator_reproduces_the_committed_maps() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/maps");
        for map in [flatlands(), river_valley()] {
            let dir = root.join(&map.meta.name);
            assert_eq!(
                fs::read(dir.join("height.png")).unwrap(),
                png16_bytes(&map.height),
                "{} height",
                map.meta.name
            );
            assert_eq!(
                fs::read(dir.join("water.png")).unwrap(),
                png16_bytes(&map.water),
                "{} water",
                map.meta.name
            );
            let ron =
                ron::ser::to_string_pretty(&map.meta, ron::ser::PrettyConfig::default()).unwrap();
            assert_eq!(
                fs::read_to_string(dir.join("map.ron")).unwrap(),
                ron,
                "{} meta",
                map.meta.name
            );
        }
    }

    /// Steepest slope and largest one-sample height step along the borders of the meadows
    /// (`0.02 < meadow < 0.98`), and the largest step anywhere on river_valley.
    fn border_stats(map: &Map) -> (f32, f32, f32) {
        let entry = valley_entry_path();
        let sl = slopes(&map.height);
        let (mut border, mut jump_border, mut jump_all) = (0.0f32, 0.0f32, 0.0f32);
        for iz in 0..GRID {
            for ix in 0..GRID {
                let (x, z) = (ix as f32 * SPACING, iz as f32 * SPACING);
                let m = meadow(x, z, &entry);
                let s = sl[iz * GRID + ix];
                jump_all = jump_all.max(s * SPACING);
                if m > 0.02 && m < 0.98 {
                    border = border.max(s);
                    jump_border = jump_border.max(s * SPACING);
                }
            }
        }
        (border, jump_border, jump_all)
    }

    /// T-180: the meadows blend into the hills over a wide band: no cliff wall or step along
    /// their borders, and no step above 3 m between two samples anywhere.
    #[test]
    fn meadow_borders_blend_into_the_hills_without_cliffs() {
        let map = river_valley();
        let (slope, border_step, any_step) = border_stats(&map);
        println!(
            "border slope {slope:.3}, border step {border_step:.2} m, max step {any_step:.2} m"
        );
        assert!(slope <= 0.25, "meadow border slope {slope}");
        assert!(
            border_step <= 2.5,
            "step along a meadow border {border_step} m"
        );
        assert!(any_step <= 3.0, "step between two samples {any_step} m");
    }
}
