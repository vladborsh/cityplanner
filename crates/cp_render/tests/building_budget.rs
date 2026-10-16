//! T-113: render budget of the building looks on a built grid city, plus plan and oblique
//! rasters of a few streets (so the variety can be seen without a window).
//!
//! `#[ignore]`d: they build a grid city (10k: seconds in release, 100k: about 80 s). Run
//! `cargo test -p cp_render --release --test building_budget -- --ignored --nocapture`;
//! `BUDGET_OUT=<dir>` is where the PNGs go (default `$CARGO_TARGET_DIR/building-rasters`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use bevy_math::{Vec2, Vec3};
use cp_content::{ContentDb, ZoneType};
use cp_render::building::{BUILDING_CHUNK, build_building, chunk_of};
use cp_render::chunk::MeshData;
use cp_sim::scenario::{GridCityConfig, grid_city};
use cp_terrain::Terrain;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");
/// Per-def vertex counts (near, mid, far) of the plain-box buildings before T-111.
const BASELINE: &str = include_str!("../src/building/baseline_counts.txt");

fn content() -> Arc<ContentDb> {
    Arc::new(ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap())
}

fn terrain() -> Arc<Terrain> {
    Arc::new(Terrain::load(&Path::new(ROOT).join("../../assets/maps/flatlands")).unwrap())
}

/// Bytes of one vertex: position, normal (3 × f32 each) and colour (4 × f32).
const VERTEX_BYTES: usize = 40;

#[derive(Default)]
struct Totals {
    buildings: usize,
    verts: [usize; 3],
    tris: [usize; 3],
    /// What the same buildings had as plain boxes (their def's pre-T-111 counts).
    base_verts: [usize; 3],
    by_zone: BTreeMap<String, ([usize; 3], [usize; 3], usize)>,
}

fn measure(pop: usize) -> (Totals, usize, cp_sim::SimView) {
    let content = content();
    let (sim, report) = grid_city(
        pop,
        1,
        terrain(),
        Arc::clone(&content),
        &GridCityConfig::default(),
    );
    assert!(report.reached, "{report:?}");
    let view = sim.view();
    let grid = view.buildings.clone();
    let terrain = view.terrain.clone().unwrap();
    let base: BTreeMap<&str, [usize; 3]> = BASELINE
        .lines()
        .map(|l| {
            let v: Vec<&str> = l.split(' ').collect();
            (v[0], [1, 2, 3].map(|k| v[k].parse().unwrap()))
        })
        .collect();
    let mut t = Totals::default();
    let mut chunks = std::collections::BTreeSet::new();
    for (id, b) in grid.iter() {
        let def = content.building(b.def);
        let p = build_building(id, b, def, &view.net, &terrain, &content, cp_core::Tick(0));
        let v = [
            p.near.positions.len(),
            p.mid.positions.len(),
            p.far.positions.len(),
        ];
        let tr = [
            p.near.triangle_count(),
            p.mid.triangle_count(),
            p.far.triangle_count(),
        ];
        let bl = base.get(def.id.as_str()).copied().unwrap_or(v);
        t.buildings += 1;
        chunks.insert(chunk_of(p.center()));
        let z = t.by_zone.entry(format!("{:?}", def.zone)).or_default();
        z.2 += 1;
        for k in 0..3 {
            t.verts[k] += v[k];
            t.tris[k] += tr[k];
            t.base_verts[k] += bl[k];
            z.0[k] += v[k];
            z.1[k] += bl[k];
        }
    }
    (t, chunks.len(), view)
}

fn report(pop: usize, t: &Totals, chunks: usize) {
    println!(
        "== grid_city {pop}: {} buildings in {chunks} chunks of {BUILDING_CHUNK} m",
        t.buildings
    );
    for (k, lod) in ["near", "mid", "far"].iter().enumerate() {
        println!(
            "{lod}: {} vertices ({:.1} MB), {} triangles; avg {:.0} vertices/building; plain-box baseline {} -> {:.2}x",
            t.verts[k],
            (t.verts[k] * VERTEX_BYTES) as f64 / 1e6,
            t.tris[k],
            t.verts[k] as f64 / t.buildings as f64,
            t.base_verts[k],
            t.verts[k] as f64 / t.base_verts[k] as f64
        );
    }
    for (z, (v, b, n)) in &t.by_zone {
        println!(
            "  {z:<11} {n:>6} buildings  near {:>6.0} ({:.2}x)  mid {:>5.0} ({:.2}x)  far {:>4.0} ({:.2}x)",
            v[0] as f64 / *n as f64,
            v[0] as f64 / b[0] as f64,
            v[1] as f64 / *n as f64,
            v[1] as f64 / b[1] as f64,
            v[2] as f64 / *n as f64,
            v[2] as f64 / b[2] as f64,
        );
    }
}

#[test]
#[ignore = "builds a 10k grid city; run in release"]
fn budget_and_rasters_10k() {
    let (t, chunks, view) = measure(10_000);
    report(10_000, &t, chunks);
    assert!(
        t.verts[0] as f64 <= 1.5 * t.base_verts[0] as f64,
        "near over 1.5x"
    );
    assert!(
        t.verts[1] as f64 <= 1.2 * t.base_verts[1] as f64,
        "mid over 1.2x"
    );
    assert!(
        t.verts[2] as f64 <= 1.2 * t.base_verts[2] as f64,
        "far over 1.2x"
    );
    let out = std::env::var("BUDGET_OUT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(std::env::var("CARGO_TARGET_DIR").unwrap_or("target".into()))
                .join("building-rasters")
        });
    std::fs::create_dir_all(&out).unwrap();
    rasters(&view, &out);
}

#[test]
#[ignore = "builds a 100k grid city (about 80 s in release)"]
fn budget_100k() {
    let (t, chunks, _) = measure(100_000);
    report(100_000, &t, chunks);
    assert!(
        t.verts[0] as f64 <= 1.5 * t.base_verts[0] as f64,
        "near over 1.5x"
    );
    assert!(
        t.verts[1] as f64 <= 1.2 * t.base_verts[1] as f64,
        "mid over 1.2x"
    );
    assert!(
        t.verts[2] as f64 <= 1.2 * t.base_verts[2] as f64,
        "far over 1.2x"
    );
}

// ---------------------------------------------------------------------------------------
// Rasters

struct Cam {
    right: Vec3,
    up: Vec3,
    dir: Vec3,
    centre: Vec3,
    px_per_m: f32,
    w: usize,
    h: usize,
}

impl Cam {
    fn new(centre: Vec3, yaw_deg: f32, pitch_deg: f32, px_per_m: f32, w: usize, h: usize) -> Self {
        let (a, p) = (yaw_deg.to_radians(), pitch_deg.to_radians());
        let dir = Vec3::new(-p.cos() * a.sin(), -p.sin(), -p.cos() * a.cos()).normalize();
        let c = dir.cross(Vec3::Y);
        let right = if c.length() > 1e-3 {
            c.normalize()
        } else {
            Vec3::X
        };
        let up = right.cross(dir);
        Self {
            right,
            up,
            dir,
            centre,
            px_per_m,
            w,
            h,
        }
    }
    /// Screen x, y (y down) and depth (smaller is nearer).
    fn project(&self, p: Vec3) -> (f32, f32, f32) {
        let d = p - self.centre;
        (
            self.w as f32 * 0.5 + d.dot(self.right) * self.px_per_m,
            self.h as f32 * 0.5 - d.dot(self.up) * self.px_per_m,
            d.dot(self.dir),
        )
    }
}

struct Image {
    w: usize,
    h: usize,
    rgb: Vec<u8>,
    depth: Vec<f32>,
}

impl Image {
    fn new(w: usize, h: usize, bg: [f32; 3]) -> Self {
        let px = bg.map(|c| (c * 255.0) as u8);
        Self {
            w,
            h,
            rgb: (0..w * h).flat_map(|_| px).collect(),
            depth: vec![f32::MAX; w * h],
        }
    }

    fn tri(&mut self, cam: &Cam, p: [Vec3; 3], col: [f32; 3]) {
        let s = p.map(|p| cam.project(p));
        let area = (s[1].0 - s[0].0) * (s[2].1 - s[0].1) - (s[2].0 - s[0].0) * (s[1].1 - s[0].1);
        if area.abs() < 1e-6 {
            return;
        }
        let (x0, x1) = (
            s.iter()
                .map(|v| v.0)
                .fold(f32::MAX, f32::min)
                .floor()
                .max(0.0) as usize,
            (s.iter()
                .map(|v| v.0)
                .fold(f32::MIN, f32::max)
                .ceil()
                .max(0.0) as usize)
                .min(self.w - 1),
        );
        let (y0, y1) = (
            s.iter()
                .map(|v| v.1)
                .fold(f32::MAX, f32::min)
                .floor()
                .max(0.0) as usize,
            (s.iter()
                .map(|v| v.1)
                .fold(f32::MIN, f32::max)
                .ceil()
                .max(0.0) as usize)
                .min(self.h - 1),
        );
        for y in y0..=y1 {
            for x in x0..=x1 {
                let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                let w0 = ((s[1].0 - px) * (s[2].1 - py) - (s[2].0 - px) * (s[1].1 - py)) / area;
                let w1 = ((s[2].0 - px) * (s[0].1 - py) - (s[0].0 - px) * (s[2].1 - py)) / area;
                let w2 = 1.0 - w0 - w1;
                if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                    continue;
                }
                let z = w0 * s[0].2 + w1 * s[1].2 + w2 * s[2].2;
                let i = y * self.w + x;
                if z < self.depth[i] {
                    self.depth[i] = z;
                    for (k, c) in col.iter().enumerate() {
                        self.rgb[i * 3 + k] = (c.clamp(0.0, 1.0) * 255.0) as u8;
                    }
                }
            }
        }
    }

    fn mesh(&mut self, cam: &Cam, m: &MeshData) {
        let sun = Vec3::new(-0.45, 0.8, -0.4).normalize();
        for t in m.indices.chunks(3) {
            let p = |k: u32| Vec3::from_array(m.positions[k as usize]);
            let n = Vec3::from_array(m.normals[t[0] as usize]);
            let c = m.colors[t[0] as usize];
            let lit = 0.45 + 0.55 * n.dot(sun).max(0.0);
            self.tri(
                cam,
                [p(t[0]), p(t[1]), p(t[2])],
                [c[0] * lit, c[1] * lit, c[2] * lit],
            );
        }
    }

    fn save(&self, path: &Path) {
        let file = std::fs::File::create(path).unwrap();
        let mut enc =
            png::Encoder::new(std::io::BufWriter::new(file), self.w as u32, self.h as u32);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header()
            .unwrap()
            .write_image_data(&self.rgb)
            .unwrap();
    }
}

/// Road ribbons as flat quads, so the buildings sit on streets.
fn roads(view: &cp_sim::SimView, content: &ContentDb, near: Vec2, r: f32) -> MeshData {
    let mut m = MeshData::default();
    for (_, s) in view.net.segments() {
        let half = content.road(s.road).width * 0.5;
        let pts: Vec<Vec3> = (0..=16).map(|k| s.curve.point(k as f32 / 16.0)).collect();
        if pts.iter().all(|p| Vec2::new(p.x, p.z).distance(near) > r) {
            continue;
        }
        for w in pts.windows(2) {
            let d = Vec3::new(w[1].x - w[0].x, 0.0, w[1].z - w[0].z).normalize_or_zero();
            let side = Vec3::new(-d.z, 0.0, d.x) * half;
            let y = 0.05;
            let q = [w[0] - side, w[0] + side, w[1] + side, w[1] - side]
                .map(|p| Vec3::new(p.x, p.y + y, p.z));
            let base = m.positions.len() as u32;
            for p in q {
                m.positions.push(p.to_array());
                m.normals.push([0.0, 1.0, 0.0]);
                m.colors.push([0.30, 0.30, 0.32, 1.0]);
            }
            m.indices
                .extend_from_slice(&[base, base + 2, base + 1, base, base + 3, base + 2]);
        }
    }
    m
}

/// The densest 48 m cell of `zones` (so the picture is of a street, not a lone house).
fn densest(view: &cp_sim::SimView, content: &ContentDb, zones: &[ZoneType]) -> Option<Vec2> {
    let mut cells = BTreeMap::<(i32, i32), (usize, Vec2)>::new();
    for (_, b) in view.buildings.iter() {
        if zones.contains(&content.building(b.def).zone) {
            let e = cells
                .entry(((b.pos.x / 48.0) as i32, (b.pos.y / 48.0) as i32))
                .or_default();
            e.0 += 1;
            e.1 += b.pos;
        }
    }
    cells
        .values()
        .max_by_key(|(n, _)| *n)
        .map(|(n, s)| *s / *n as f32)
}

fn rasters(view: &cp_sim::SimView, out: &Path) {
    let content = content();
    let terrain = view.terrain.clone().unwrap();
    let scenes: [(&str, &[ZoneType], f32); 5] = [
        ("res_low", &[ZoneType::ResLow], 8.0),
        ("res_high", &[ZoneType::ResHigh], 5.0),
        ("com", &[ZoneType::ComLow, ZoneType::ComHigh], 7.0),
        ("industrial", &[ZoneType::Industrial], 8.0),
        ("office", &[ZoneType::Office], 5.0),
    ];
    for (name, zones, ppm) in scenes {
        let Some(c) = densest(view, &content, zones) else {
            println!("no {name} buildings");
            continue;
        };
        let centre = Vec3::new(c.x, terrain.height(c), c.y);
        for (kind, yaw, pitch, w, h) in [
            ("plan", 0.0, 90.0, 1000, 640),
            ("oblique", 35.0, 38.0, 1000, 640),
        ] {
            let cam = Cam::new(centre, yaw, pitch, ppm, w, h);
            let mut img = Image::new(w, h, [0.36, 0.45, 0.29]);
            img.mesh(&cam, &roads(view, &content, c, 260.0));
            for (id, b) in view.buildings.iter() {
                if b.pos.distance(c) > 1000.0 / ppm * 0.8 {
                    continue;
                }
                let def = content.building(b.def);
                let p = build_building(id, b, def, &view.net, &terrain, &content, cp_core::Tick(0));
                img.mesh(&cam, &p.near);
            }
            img.save(&out.join(format!("{name}-{kind}.png")));
        }
        println!("raster {name} at {c:?}");
    }
}

// ---------------------------------------------------------------------------------------
// T-114: the six service silhouettes

#[test]
#[ignore = "writes PNGs; run in release"]
fn service_rasters() {
    service_rasters_of(
        &[
            "water_pump_station",
            "sewage_outflow_station",
            "coal_power_plant",
            "wind_turbine",
            "water_tower",
            "sewage_treatment_plant",
        ],
        "services-row",
    );
}

/// T-147: the eleven M2 service silhouettes, one picture each and a row.
#[test]
#[ignore = "writes PNGs; run in release"]
fn m2_service_rasters() {
    service_rasters_of(
        &[
            "landfill",
            "incinerator",
            "clinic",
            "hospital",
            "cemetery",
            "crematorium",
            "elementary_school",
            "high_school",
            "university",
            "fire_station",
            "police_station",
        ],
        "services-m2-row",
    );
}

fn service_rasters_of(names: &[&str], row: &str) {
    use cp_building::{Building, BuildingFlags, BuildingState, Footprint};
    use cp_net::NetGraph;
    let content = content();
    let terrain = terrain();
    let net = NetGraph::new();
    let out = std::env::var("BUDGET_OUT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(std::env::var("CARGO_TARGET_DIR").unwrap_or("target".into()))
                .join("building-rasters")
        });
    std::fs::create_dir_all(&out).unwrap();
    let origin = Vec2::new(5120.0, 5120.0);
    let mut all = Vec::new();
    for (k, name) in names.iter().enumerate() {
        let (def_id, def) = content.buildings().find(|(_, d)| d.id == *name).unwrap();
        let b = Building {
            def: def_id,
            footprint: Footprint::Free {
                w: def.lot.0,
                d: def.lot.1,
            },
            level: 1,
            state: BuildingState::Active,
            ready_at: cp_core::Tick(0),
            pos: origin + Vec2::new((k as f32 - (names.len() as f32 - 1.0) / 2.0) * 36.0, 0.0),
            rot: 0.0,
            access: None,
            flags: BuildingFlags::default(),
            level_progress: 0.0,
        };
        let id = cp_core::Id::new(k as u32 + 1, 0);
        let piece = build_building(id, &b, def, &net, &terrain, &content, cp_core::Tick(0));
        let mut meshes = vec![piece.near.clone()];
        if let Some(r) = piece.rotor {
            meshes.push(cp_render::building::rotor_world(&r, 0.35));
        }
        let ground = terrain.height(b.pos);
        // One close picture per building (oblique, 9 px/m) ...
        let cam = Cam::new(
            Vec3::new(b.pos.x, ground + 8.0, b.pos.y),
            35.0,
            32.0,
            9.0,
            560,
            480,
        );
        let mut img = Image::new(560, 480, [0.36, 0.45, 0.29]);
        for m in &meshes {
            img.mesh(&cam, m);
        }
        img.save(&out.join(format!("service-{name}.png")));
        all.extend(meshes);
    }
    // ... and the row from about 200 m up, oblique and plan.
    let centre = Vec3::new(origin.x, terrain.height(origin) + 5.0, origin.y);
    for (kind, yaw, pitch, ppm) in [("oblique", 20.0, 40.0, 4.2), ("plan", 0.0, 90.0, 4.2)] {
        // A longer row is drawn smaller to stay in the picture.
        let ppm = ppm * (6.0 / names.len() as f32).min(1.0);
        let cam = Cam::new(centre, yaw, pitch, ppm, 1000, 420);
        let mut img = Image::new(1000, 420, [0.36, 0.45, 0.29]);
        for m in &all {
            img.mesh(&cam, m);
        }
        img.save(&out.join(format!("{row}-{kind}.png")));
    }
}
