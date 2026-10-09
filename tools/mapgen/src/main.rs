//! Deterministic map generator: `cargo run -p mapgen [-- <out-dir>]`.
//! Writes `flatlands` and `river_valley` (map.ron, height.png, water.png) under
//! `assets/maps/` by default. Fixed seeds; output is committed, tests never regenerate it.

use std::fs::{self, File};
use std::io::BufWriter;
use std::path::{Path, PathBuf};

use cp_terrain::{GRID, MAX_HEIGHT, MapMeta, SPACING};

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
        },
        height,
        water: vec![0; GRID * GRID],
    }
}

/// Hills with a meandering river running north to south.
fn river_valley() -> Map {
    let mut height = vec![0u16; GRID * GRID];
    let mut water = vec![0u16; GRID * GRID];
    for iz in 0..GRID {
        for ix in 0..GRID {
            let (x, z) = (ix as f32 * SPACING, iz as f32 * SPACING);
            let cx = 5120.0 + 1400.0 * (z / 1900.0).sin() + 500.0 * (z / 700.0 + 1.3).sin();
            let d = (x - cx).abs();
            let floor = 80.0 - z * 0.004;
            let hills = 140.0 * fbm(x, z, 1800.0, SEED + 30) + 20.0 * fbm(x, z, 250.0, SEED + 40);
            let carve = smoothstep(30.0, 140.0, d);
            let span = smoothstep(120.0, 1600.0, d);
            let ground =
                floor - 4.0 + (4.0 + 6.0 * fbm(x, z, 400.0, SEED + 50) + hills * span) * carve;
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
        },
        height,
        water,
    }
}

fn write_png16(path: &Path, data: &[u16]) {
    let mut enc = png::Encoder::new(
        BufWriter::new(File::create(path).unwrap()),
        GRID as u32,
        GRID as u32,
    );
    enc.set_color(png::ColorType::Grayscale);
    enc.set_depth(png::BitDepth::Sixteen);
    let mut w = enc.write_header().unwrap();
    let bytes: Vec<u8> = data.iter().flat_map(|v| v.to_be_bytes()).collect();
    w.write_image_data(&bytes).unwrap();
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
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/maps"));
    for map in [flatlands(), river_valley()] {
        write_map(&root, &map);
    }
}
