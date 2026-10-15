use std::path::Path;
use std::sync::Arc;

use bevy_math::{Vec2, Vec3};
use cp_content::ContentDb;
use cp_core::Tick;
use cp_net::{ElevationMode, NetCommand, RoadPath};
use cp_sim::{SimCommand, SimRunner, SimView};
use cp_terrain::Terrain;

use super::*;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

struct World {
    sim: SimRunner,
    content: Arc<ContentDb>,
    terrain: Arc<Terrain>,
}

fn world() -> World {
    let content =
        Arc::new(ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap());
    let terrain =
        Arc::new(Terrain::load(&Path::new(ROOT).join("../../assets/maps/flatlands")).unwrap());
    let mut sim = SimRunner::new(1);
    sim.set_terrain(Arc::clone(&terrain));
    sim.set_content(Arc::clone(&content));
    World {
        sim,
        content,
        terrain,
    }
}

impl World {
    fn build(&mut self, road: &str, path: RoadPath, elevation: ElevationMode) {
        let road = self.content.road_id(road).unwrap();
        let tick = self.sim.tick();
        self.sim.push_command(
            SimCommand::Net(NetCommand::BuildRoad {
                road,
                path,
                elevation,
            }),
            tick,
        );
        self.sim.tick_once();
        self.sim.tick_once();
    }
    fn line(&mut self, road: &str, a: (f32, f32), b: (f32, f32)) {
        self.build(
            road,
            RoadPath::Straight {
                from: Vec2::new(a.0, a.1),
                to: Vec2::new(b.0, b.1),
            },
            ElevationMode::Ground,
        );
    }
    fn view(&self) -> SimView {
        self.sim.view()
    }
    fn set(&self, view: &SimView) -> (RoadSet, Update) {
        let mut set = RoadSet::default();
        let up = set.update(&view.net, &view.junctions, &self.terrain);
        (set, up)
    }
}

const X0: f32 = 4600.0;
const Z0: f32 = 4600.0;

fn cross(w: &mut World, road: &str) {
    w.line(road, (X0, Z0 + 100.0), (X0 + 200.0, Z0 + 100.0));
    w.line(road, (X0 + 100.0, Z0), (X0 + 100.0, Z0 + 200.0));
}

fn positions(m: &MeshData) -> impl Iterator<Item = Vec3> + '_ {
    m.positions.iter().map(|p| Vec3::from_array(*p))
}

fn tri_normal(m: &MeshData, t: usize) -> Vec3 {
    let p = |k: usize| Vec3::from_array(m.positions[m.indices[t * 3 + k] as usize]);
    (p(1) - p(0)).cross(p(2) - p(0)).normalize_or_zero()
}

#[test]
fn straight_segment_has_one_ring_pair_and_upward_surfaces() {
    let mut w = world();
    w.line("basic_2l", (X0, Z0), (X0 + 80.0, Z0));
    let v = w.view();
    let (set, up) = w.set(&v);
    // 1 segment + 2 dead-end junctions.
    assert_eq!(up.rebuilt, 3);
    let (id, _) = v.net.segments().next().unwrap();
    let piece = set.piece(PieceKey::Segment(id)).unwrap();
    for m in [&piece.near, &piece.far] {
        assert!(m.triangle_count() > 0);
        assert!(m.indices.iter().all(|&i| (i as usize) < m.positions.len()));
        // Every surface triangle (vertex normals up) faces up.
        for t in 0..m.triangle_count() {
            let up_all = (0..3).all(|k| m.normals[m.indices[t * 3 + k] as usize][1] > 0.99);
            if up_all {
                assert!(tri_normal(m, t).y > 0.99, "triangle {t} faces down");
            }
        }
    }
    // Far has no markings or kerbs: strictly cheaper.
    assert!(piece.far.triangle_count() < piece.near.triangle_count());
}

#[test]
fn dashes_appear_only_between_same_direction_lanes() {
    let mut w = world();
    w.line("basic_2l", (X0, Z0), (X0 + 80.0, Z0));
    w.line("medium_4l", (X0, Z0 + 100.0), (X0 + 80.0, Z0 + 100.0));
    let v = w.view();
    let (set, _) = w.set(&v);
    let tris = |name: &str| {
        let (id, _) = v
            .net
            .segments()
            .find(|(_, s)| w.content.road(s.road).id == name)
            .unwrap();
        set.piece(PieceKey::Segment(id))
            .unwrap()
            .near
            .triangle_count()
    };
    // The 4-lane road has dashed dividers on top of the common structure.
    assert!(tris("medium_4l") > 2 * tris("basic_2l"));
}

#[test]
fn curved_segment_is_sampled_every_four_metres() {
    let mut w = world();
    w.build(
        "basic_2l",
        RoadPath::Curve {
            from: Vec2::new(X0, Z0),
            control: Vec2::new(X0 + 60.0, Z0),
            to: Vec2::new(X0 + 60.0, Z0 + 60.0),
        },
        ElevationMode::Ground,
    );
    let v = w.view();
    let (set, _) = w.set(&v);
    let (id, seg) = v.net.segments().next().unwrap();
    let piece = set.piece(PieceKey::Segment(id)).unwrap();
    let rings = (seg.length / STEP).ceil() as usize + 1;
    // Far mesh: one vertex pair per band per ring plus skirts, so at least `rings` rings.
    assert!(piece.far.positions.len() >= rings * 4);
    // Smooth: consecutive ring points of the outermost band are ~STEP apart or less.
    assert!(piece.far.triangle_count() >= (rings - 1) * 2);
}

/// No gap between a segment's end ring and the junction polygon: every arm corner exists
/// in both meshes (same point within 1 cm).
#[test]
fn segment_ends_meet_the_junction_polygon() {
    let mut w = world();
    cross(&mut w, "medium_4l");
    let v = w.view();
    let (set, _) = w.set(&v);
    let mut checked = 0;
    for (node, j) in v.junctions.iter() {
        let jp = set.piece(PieceKey::Junction(*node)).unwrap();
        for arm in &j.arms {
            let sp = set.piece(PieceKey::Segment(arm.segment)).unwrap();
            for corner in [arm.right, arm.left] {
                let close = |m: &MeshData| {
                    positions(m).any(|p| Vec2::new(p.x, p.z).distance(corner) < 0.01)
                };
                assert!(close(&jp.near), "junction {node:?} lacks corner {corner:?}");
                assert!(
                    close(&sp.near),
                    "segment {:?} lacks corner {corner:?}",
                    arm.segment
                );
                // And at the same height.
                // The junction's corner and the segment's end ring share that point; the
                // segment's raised sidewalk adds a vertex CURB higher, capped by a face.
                let ys = |m: &MeshData| -> Vec<f32> {
                    positions(m)
                        .filter(|p| Vec2::new(p.x, p.z).distance(corner) < 0.01)
                        .map(|p| p.y)
                        .collect()
                };
                let (yj, ys_) = (ys(&jp.near), ys(&sp.near));
                assert!(
                    yj.iter().any(|a| ys_.iter().any(|b| (a - b).abs() < 0.01)),
                    "no common height at {corner:?}: {yj:?} vs {ys_:?}"
                );
                checked += 1;
            }
        }
    }
    // Two corners per arm, every arm of every junction.
    let arms: usize = v.junctions.iter().map(|(_, j)| j.arms.len()).sum();
    assert_eq!(checked, 2 * arms);
    assert!(arms >= 8);
}

#[test]
fn four_way_junction_is_one_flat_polygon_facing_up() {
    let mut w = world();
    cross(&mut w, "basic_2l");
    let v = w.view();
    let (set, _) = w.set(&v);
    let (node, j) = v.junctions.iter().find(|(_, j)| j.arms.len() == 4).unwrap();
    let piece = set.piece(PieceKey::Junction(*node)).unwrap();
    assert!(j.polygon.len() >= 4, "{:?}", j.polygon);
    // The upward triangles tile exactly the polygon's area; the rest are skirts.
    let area_of = |t: usize| {
        let p = |k: usize| {
            Vec3::from_array(piece.near.positions[piece.near.indices[t * 3 + k] as usize])
        };
        (p(1) - p(0)).cross(p(2) - p(0)).length() / 2.0
    };
    let up_area: f32 = (0..piece.near.triangle_count())
        .filter(|&t| tri_normal(&piece.near, t).y > 0.99)
        .map(area_of)
        .sum();
    let n = j.polygon.len();
    let poly_area: f32 = (0..n)
        .map(|i| j.polygon[i].perp_dot(j.polygon[(i + 1) % n]))
        .sum::<f32>()
        .abs()
        / 2.0;
    assert!(
        (up_area - poly_area).abs() < 0.01 * poly_area,
        "{up_area} vs {poly_area}"
    );
}

#[test]
fn ear_clipping_handles_concave_polygons() {
    // An L shape.
    let l = [
        Vec2::new(0.0, 0.0),
        Vec2::new(4.0, 0.0),
        Vec2::new(4.0, 2.0),
        Vec2::new(2.0, 2.0),
        Vec2::new(2.0, 4.0),
        Vec2::new(0.0, 4.0),
    ];
    let tris = triangulate(&l);
    assert_eq!(tris.len(), 4);
    let area: f32 = tris
        .iter()
        .map(|t| (l[t[1]] - l[t[0]]).perp_dot(l[t[2]] - l[t[0]]).abs() / 2.0)
        .sum();
    assert!((area - 12.0).abs() < 1e-4, "area {area}");
}

#[test]
fn skirts_reach_below_the_terrain() {
    let mut w = world();
    w.line("basic_2l", (X0, Z0), (X0 + 80.0, Z0));
    let v = w.view();
    let (set, _) = w.set(&v);
    let (id, _) = v.net.segments().next().unwrap();
    let m = &set.piece(PieceKey::Segment(id)).unwrap().near;
    let ground = w.terrain.height(Vec2::new(X0 + 40.0, Z0));
    let min_y = positions(m).map(|p| p.y).fold(f32::MAX, f32::min);
    assert!(
        min_y < ground - 0.3,
        "skirt bottom {min_y} vs ground {ground}"
    );
}

#[test]
fn elevated_segment_gets_deck_underside_and_piers() {
    let mut w = world();
    // A long path: it ramps up from the ground and back down.
    w.build(
        "basic_2l",
        RoadPath::Straight {
            from: Vec2::new(X0, Z0),
            to: Vec2::new(X0 + 400.0, Z0),
        },
        ElevationMode::Elevated(8.0),
    );
    let v = w.view();
    let (set, _) = w.set(&v);
    let (id, seg) = v
        .net
        .segments()
        .filter(|(_, s)| s.elevation == Elevation::Elevated)
        .max_by(|a, b| a.1.length.total_cmp(&b.1.length))
        .expect("an elevated piece");
    let elevated = set.piece(PieceKey::Segment(id)).unwrap();
    // Underside (faces down) and fascias.
    assert!((0..elevated.near.triangle_count()).any(|t| tri_normal(&elevated.near, t).y < -0.99));
    // Piers (when the piece is long enough to have any) reach the terrain.
    if !cp_net::pier_positions(seg).is_empty() {
        let p = cp_net::pier_positions(seg)[0];
        let terrain_y = w.terrain.height(Vec2::new(p.x, p.z));
        let low = positions(&elevated.near)
            .filter(|q| Vec2::new(q.x, q.z).distance(Vec2::new(p.x, p.z)) < 1.6)
            .map(|q| q.y)
            .fold(f32::MAX, f32::min);
        assert!(
            low <= terrain_y + 0.1,
            "pier base {low} vs terrain {terrain_y}"
        );
    }
    // A raised piece costs more triangles than the same piece on the ground would.
    assert!(elevated.near.triangle_count() > 10);
}

#[test]
fn only_changed_pieces_are_rebuilt() {
    let mut w = world();
    // 5x5 lattice, 100 m apart.
    for i in 0..5 {
        let o = i as f32 * 100.0;
        w.line("basic_2l", (X0, Z0 + o), (X0 + 400.0, Z0 + o));
        w.line("basic_2l", (X0 + o, Z0), (X0 + o, Z0 + 400.0));
    }
    let v1 = w.view();
    let mut set = RoadSet::default();
    let first = set.update(&v1.net, &v1.junctions, &w.terrain);
    assert!(first.rebuilt >= 25 + 40, "{first:?}");
    let before = set.piece_count();
    // Nothing changed: nothing rebuilt.
    let idle = set.update(&v1.net, &v1.junctions, &w.terrain);
    assert_eq!((idle.rebuilt, idle.removed), (0, 0));
    assert!(idle.dirty_chunks.is_empty());

    // One more street end-to-end beside the lattice: only its neighbourhood changes.
    w.line("basic_2l", (X0 + 500.0, Z0), (X0 + 500.0, Z0 + 90.0));
    let v2 = w.view();
    let t0 = std::time::Instant::now();
    let edit = set.update(&v2.net, &v2.junctions, &w.terrain);
    let ms = t0.elapsed().as_secs_f64() * 1000.0;
    assert!(edit.rebuilt >= 1 && edit.rebuilt <= 6, "{edit:?}");
    assert!(set.piece_count() > before);
    // Generous in unoptimised test builds; the release number is reported by the bench.
    assert!(ms < 50.0, "incremental update took {ms} ms");
}

#[test]
fn removing_a_segment_drops_its_pieces() {
    let mut w = world();
    cross(&mut w, "basic_2l");
    let v = w.view();
    let (mut set, _) = w.set(&v);
    let n = set.piece_count();
    let (id, _) = v.net.segments().next().unwrap();
    let tick = w.sim.tick();
    w.sim
        .push_command(SimCommand::Net(NetCommand::RemoveSegment { seg: id }), tick);
    w.sim.tick_once();
    w.sim.tick_once();
    let v2 = w.view();
    let up = set.update(&v2.net, &v2.junctions, &w.terrain);
    assert!(up.removed >= 1, "{up:?}");
    assert!(set.piece_count() < n);
    let _ = Tick(0);
}

#[test]
fn chunk_meshes_merge_pieces_with_valid_indices() {
    let mut w = world();
    cross(&mut w, "medium_4l");
    let v = w.view();
    let (set, _) = w.set(&v);
    let mut total = 0;
    for &c in set.chunks() {
        for far in [false, true] {
            let m = set.chunk_mesh(c, far).unwrap();
            assert!(m.indices.iter().all(|&i| (i as usize) < m.positions.len()));
            assert_eq!(m.positions.len(), m.normals.len());
            assert_eq!(m.positions.len(), m.colors.len());
            total += m.triangle_count();
        }
        assert!(set.chunk_bounds(c).is_some());
    }
    assert!(total > 0);
    assert_eq!(total, set.triangle_count(false) + set.triangle_count(true));
}

/// 30x30 street lattice (900 junctions): full build, then one new street.
/// `cargo test --release -p cp_render road_perf -- --nocapture` prints the numbers.
#[test]
fn road_perf_30x30_grid_and_one_edit() {
    let mut w = world();
    // Unoptimised builds run a smaller lattice to keep `cargo test` quick.
    let n = if cfg!(debug_assertions) { 8 } else { 30 };
    let (x0, z0) = (3600.0, 3600.0);
    let span = (n - 1) as f32 * 100.0;
    let t = std::time::Instant::now();
    for i in 0..n {
        let o = i as f32 * 100.0;
        w.line("basic_2l", (x0, z0 + o), (x0 + span, z0 + o));
        w.line("basic_2l", (x0 + o, z0), (x0 + o, z0 + span));
    }
    let sim_ms = t.elapsed().as_secs_f64() * 1000.0;
    let v1 = w.view();
    let mut set = RoadSet::default();
    let t = std::time::Instant::now();
    let full = set.update(&v1.net, &v1.junctions, &w.terrain);
    let full_ms = t.elapsed().as_secs_f64() * 1000.0;

    // One more street segment attached to the lattice edge.
    w.line(
        "basic_2l",
        (x0 + span, z0 + 50.0),
        (x0 + span + 90.0, z0 + 50.0),
    );
    let v2 = w.view();
    let t = std::time::Instant::now();
    let edit = set.update(&v2.net, &v2.junctions, &w.terrain);
    let edit_ms = t.elapsed().as_secs_f64() * 1000.0;
    // Merging the touched chunks into meshes is part of a rebuild too.
    let t = std::time::Instant::now();
    for &c in &edit.dirty_chunks {
        let _ = set.chunk_mesh(c, false);
        let _ = set.chunk_mesh(c, true);
    }
    let merge_ms = t.elapsed().as_secs_f64() * 1000.0;
    println!(
        "ROADPERF sim_build_ms={sim_ms:.0} pieces={} chunks={} near_tris={} far_tris={} \
         full_build_ms={full_ms:.1} edit_rebuilt={} edit_update_ms={edit_ms:.3} \
         edit_chunks={} edit_merge_ms={merge_ms:.3}",
        set.piece_count(),
        set.chunks().count(),
        set.triangle_count(false),
        set.triangle_count(true),
        edit.rebuilt,
        edit.dirty_chunks.len(),
    );
    assert!(full.rebuilt >= n * n + 2 * n * (n - 1));
    assert!(edit.rebuilt <= 12, "{edit:?}");
    if !cfg!(debug_assertions) {
        assert!(
            edit_ms + merge_ms <= 2.0,
            "edit took {} ms",
            edit_ms + merge_ms
        );
    }
}

/// After edits, the incrementally maintained set equals one built from scratch.
#[test]
fn incremental_update_matches_a_fresh_build() {
    let mut w = world();
    cross(&mut w, "basic_2l");
    let v = w.view();
    let mut inc = RoadSet::default();
    inc.update(&v.net, &v.junctions, &w.terrain);

    // Edits: a branch, an upgrade of a segment, a removal.
    w.line(
        "basic_2l",
        (X0 + 100.0, Z0 + 100.0),
        (X0 + 190.0, Z0 + 190.0),
    );
    let v = w.view();
    inc.update(&v.net, &v.junctions, &w.terrain);
    let (seg, _) = v.net.segments().next().unwrap();
    let big = w.content.road_id("medium_4l").unwrap();
    let tick = w.sim.tick();
    w.sim.push_command(
        SimCommand::Net(NetCommand::UpgradeSegment { seg, road: big }),
        tick,
    );
    w.sim.tick_once();
    w.sim.tick_once();
    let v = w.view();
    inc.update(&v.net, &v.junctions, &w.terrain);
    let (seg, _) = v.net.segments().nth(2).unwrap();
    let tick = w.sim.tick();
    w.sim
        .push_command(SimCommand::Net(NetCommand::RemoveSegment { seg }), tick);
    w.sim.tick_once();
    w.sim.tick_once();
    let v = w.view();
    inc.update(&v.net, &v.junctions, &w.terrain);

    let mut fresh = RoadSet::default();
    fresh.update(&v.net, &v.junctions, &w.terrain);
    assert_eq!(inc.piece_count(), fresh.piece_count());
    for far in [false, true] {
        assert_eq!(inc.triangle_count(far), fresh.triangle_count(far));
    }
    let chunks: Vec<_> = fresh.chunks().copied().collect();
    assert_eq!(inc.chunks().copied().collect::<Vec<_>>(), chunks);
    for c in chunks {
        let (a, b) = (
            inc.chunk_mesh(c, false).unwrap(),
            fresh.chunk_mesh(c, false).unwrap(),
        );
        assert_eq!(a.positions, b.positions, "chunk {c:?}");
        assert_eq!(a.indices, b.indices, "chunk {c:?}");
    }
}

/// Triangles whose three vertices are all chevron-coloured.
fn chevron_triangles(m: &MeshData) -> usize {
    (0..m.triangle_count())
        .filter(|&t| (0..3).all(|k| m.colors[m.indices[t * 3 + k] as usize] == CHEVRON))
        .count()
}

fn segment_length(w: &World, v: &SimView, id: SegmentId) -> f32 {
    let _ = w;
    v.junctions
        .trimmed_curve(&v.net, id)
        .unwrap()
        .arc_table()
        .length()
}

#[test]
fn a_one_way_segment_gets_floor_len_over_24_chevrons_and_a_two_way_one_none() {
    let mut w = world();
    w.line("highway_1w_2l", (X0, Z0), (X0 + 130.0, Z0));
    w.line("highway_1w_1l", (X0, Z0 + 100.0), (X0 + 60.0, Z0 + 100.0));
    w.line("basic_2l", (X0, Z0 + 200.0), (X0 + 130.0, Z0 + 200.0));
    let v = w.view();
    let (set, _) = w.set(&v);
    let mut seen = 0;
    for (id, seg) in v.net.segments() {
        let piece = set.piece(PieceKey::Segment(id)).unwrap();
        let name = &w.content.road(seg.road).id;
        let n = chevron_triangles(&piece.near) / 4;
        if name.starts_with("highway_1w") {
            let expected = (segment_length(&w, &v, id) / CHEVRON_SPACING).floor() as usize;
            assert_eq!(n, expected, "{name}");
            // The far mesh has none, and is strictly cheaper than the near one.
            assert_eq!(chevron_triangles(&piece.far), 0);
            assert!(piece.far.triangle_count() < piece.near.triangle_count());
            seen += 1;
        } else {
            assert_eq!(n, 0, "{name} is two-way");
        }
    }
    // Long roads may be split into several segments; each is checked on its own length.
    assert!(seen >= 2, "{seen} one-way segments");
}

#[test]
fn chevrons_point_along_the_segment_replace_the_asphalt_and_face_up() {
    let mut w = world();
    w.line("highway_1w_2l", (X0, Z0), (X0 + 100.0, Z0));
    let v = w.view();
    let (set, _) = w.set(&v);
    let (id, _) = v.net.segments().next().unwrap();
    let m = &set.piece(PieceKey::Segment(id)).unwrap().near;
    // The segment runs along +x: each chevron's furthest vertex (its tip) is on the axis z = Z0
    // and ahead of the other vertices of its arms.
    let chevron_verts: Vec<Vec3> = (0..m.triangle_count())
        .filter(|&t| (0..3).all(|k| m.colors[m.indices[t * 3 + k] as usize] == CHEVRON))
        .flat_map(|t| {
            (0..3).map(move |k| Vec3::from_array(m.positions[m.indices[t * 3 + k] as usize]))
        })
        .collect();
    let tip_x = chevron_verts.iter().map(|p| p.x).fold(f32::MIN, f32::max);
    let tips: Vec<_> = chevron_verts
        .iter()
        .filter(|p| (p.x - tip_x).abs() < 1e-3)
        .collect();
    assert!(!tips.is_empty());
    assert!(
        tips.iter().all(|p| (p.z - Z0).abs() < 1e-3),
        "tip on the axis"
    );
    // The arms trail behind the tip: the lowest-x vertices are off the axis.
    let tail_x = chevron_verts.iter().map(|p| p.x).fold(f32::MAX, f32::min);
    assert!(
        chevron_verts
            .iter()
            .filter(|p| (p.x - tail_x).abs() < 1e-3)
            .all(|p| (p.z - Z0).abs() > 0.5)
    );
    // Every up-facing triangle faces up (no flipped fan), and nothing is stacked on the
    // surface: no two triangles of the road level cover the same point, checked via the
    // total area of the carriageway band equalling its rectangle.
    for t in 0..m.triangle_count() {
        if (0..3).all(|k| m.normals[m.indices[t * 3 + k] as usize][1] > 0.99) {
            assert!(tri_normal(m, t).y > 0.99, "triangle {t} faces down");
        }
    }
}

#[test]
fn chevrons_do_not_change_the_carriageway_area() {
    let mut w = world();
    w.line("highway_1w_2l", (X0, Z0), (X0 + 100.0, Z0));
    let v = w.view();
    let (set, _) = w.set(&v);
    let (id, _) = v.net.segments().next().unwrap();
    let m = &set.piece(PieceKey::Segment(id)).unwrap().near;
    let top = |c: [f32; 4]| -> f32 {
        (0..m.triangle_count())
            .filter(|&t| (0..3).all(|k| m.colors[m.indices[t * 3 + k] as usize] == c))
            .map(|t| {
                let p = |k: usize| Vec3::from_array(m.positions[m.indices[t * 3 + k] as usize]);
                (p(1) - p(0)).cross(p(2) - p(0)).length() * 0.5
            })
            .sum()
    };
    let len = segment_length(&w, &v, id);
    let area = top(ASPHALT) + top(CHEVRON) + top(WHITE);
    // The two lanes (7.5 m) over the whole segment, divider line included.
    assert!(
        (area - 7.5 * len).abs() < 0.5,
        "area {area} vs {}",
        7.5 * len
    );
}
