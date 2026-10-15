//! T-119: how long the zone grid takes to rebuild, without a window. Builds `grid_city` of
//! `--pop N` citizens (default 100000) on `--map NAME` (default flatlands) and prints, in ms:
//! the old whole-grid rebuild, the first chunked build, and the worst frame of a camera walk in
//! 250 m steps, of a zoning-mode flip and of a one-cell change.
//!
//! `cargo run --release -p cp_render --example zone_rebuild -- --pop 100000`

use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use bevy_math::Vec3;
use cp_content::{ContentDb, ZoneType};
use cp_render::overlay::{BUILD_BUDGET, ZoneChunks, ZoneStyle, zone_mesh_with, zone_outline_mesh};
use cp_sim::scenario::{GridCityConfig, grid_city};
use cp_terrain::Terrain;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1e3
}

/// Calls `update` until nothing is left; returns (worst frame ms, frames, chunks rebuilt).
fn drain(
    chunks: &mut ZoneChunks,
    terrain: &Terrain,
    key: usize,
    style: ZoneStyle,
) -> (f64, usize, usize) {
    let (mut worst, mut frames, mut built) = (0.0f64, 0, 0);
    loop {
        let t = Instant::now();
        let u = chunks.update(terrain, key, style, BUILD_BUDGET);
        let dt = ms(t);
        if u.is_empty() {
            return (worst, frames, built);
        }
        worst = worst.max(dt);
        frames += 1;
        built += u.len();
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let arg = |name: &str, default: &str| {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1).cloned())
            .unwrap_or(default.into())
    };
    let pop: usize = arg("--pop", "100000").parse().unwrap();
    let map = arg("--map", "flatlands");
    let terrain =
        Arc::new(Terrain::load(&Path::new(ROOT).join("../../assets/maps").join(&map)).unwrap());
    let content =
        Arc::new(ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap());
    let (sim, _) = grid_city(
        pop,
        1,
        Arc::clone(&terrain),
        content,
        &GridCityConfig::default(),
    );
    let zones = sim.view().overlays.zones.clone();
    println!("cells: {}", zones.cells.len());
    let key = Arc::as_ptr(&terrain) as usize;
    let c = cp_terrain::MAP_SIZE / 2.0;
    let cam = |x: f32, y: f32| Some(Vec3::new(c + x, y, c));
    let mut style = ZoneStyle {
        zoning: false,
        cam: cam(0.0, 300.0),
    };

    let t = Instant::now();
    let fill = zone_mesh_with(&zones, &terrain, false, style.cam);
    let fill_ms = ms(t);
    let t = Instant::now();
    let line = zone_outline_mesh(&zones, &terrain, style.cam.unwrap(), 0, false);
    println!(
        "whole grid (old): fill {fill_ms:.1} + outlines {:.1} ms ({} + {} tris)",
        ms(t),
        fill.triangle_count(),
        line.triangle_count()
    );

    let mut chunks = ZoneChunks::default();
    chunks.set_zones(&zones);
    let (worst, frames, built) = drain(&mut chunks, &terrain, key, style);
    println!("first build: {frames} frames, {built} chunks, worst frame {worst:.2} ms");

    for (name, y) in [("low", 150.0f32), ("mid", 600.0), ("high", 2500.0)] {
        let mut worst = 0.0f64;
        let mut chunks_built = 0;
        style.cam = cam(-3000.0, y);
        drain(&mut chunks, &terrain, key, style);
        for step in 0..48 {
            style.cam = cam(-3000.0 + step as f32 * 125.0, y);
            let (w, _, n) = drain(&mut chunks, &terrain, key, style);
            worst = worst.max(w);
            chunks_built += n;
        }
        println!(
            "camera walk {name} ({y} m): worst frame {worst:.2} ms, {chunks_built} chunk builds"
        );
    }

    style.zoning = true;
    let (w, f, n) = drain(&mut chunks, &terrain, key, style);
    println!("zoning on: {f} frames, {n} chunks, worst frame {w:.2} ms");
    style.zoning = false;
    let (w, f, n) = drain(&mut chunks, &terrain, key, style);
    println!("zoning off: {f} frames, {n} chunks, worst frame {w:.2} ms");

    let mut changed = zones.as_ref().clone();
    let i = changed.cells.len() / 2;
    changed.cells[i].zone = ZoneType::Office;
    chunks.set_zones(&changed);
    let (w, f, n) = drain(&mut chunks, &terrain, key, style);
    println!("one cell painted: {f} frames, {n} chunks, worst frame {w:.2} ms");
}
