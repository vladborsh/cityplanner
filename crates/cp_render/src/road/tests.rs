use std::path::Path;
use std::sync::Arc;

use bevy_math::{Vec2, Vec3};
use cp_content::{ContentDb, LaneDir, LaneKind};
use cp_core::Tick;
use cp_net::{ElevationMode, Junction, NetCommand, NodeClass, RoadPath};
use cp_sim::{SimCommand, SimRunner, SimView};
use cp_terrain::Terrain;

use super::*;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

struct World {
    sim: SimRunner,
    content: Arc<ContentDb>,
    terrain: Arc<Terrain>,
    /// Commands are applied without a tick, as while the game is paused.
    paused: bool,
}

fn world() -> World {
    world_on("flatlands")
}

fn world_on(map: &str) -> World {
    let content =
        Arc::new(ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap());
    let terrain =
        Arc::new(Terrain::load(&Path::new(ROOT).join("../../assets/maps").join(map)).unwrap());
    let mut sim = SimRunner::new(1);
    sim.set_terrain(Arc::clone(&terrain));
    sim.set_content(Arc::clone(&content));
    World {
        sim,
        content,
        terrain,
        paused: false,
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
        if self.paused {
            self.sim.apply_due_commands();
        } else {
            self.sim.tick_once();
            self.sim.tick_once();
        }
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
        let up = set.update(&view.net, &view.junctions, &self.terrain, &self.content);
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
        // A middle node has no piece: its segments meet each other (T-263).
        let Some(jp) = set.piece(PieceKey::Junction(*node)) else {
            assert_eq!(j.class, NodeClass::Middle);
            continue;
        };
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
    // Two corners per arm, every arm of every node with a piece.
    let arms: usize = v
        .junctions
        .iter()
        .filter(|(_, j)| j.class != NodeClass::Middle)
        .map(|(_, j)| j.arms.len())
        .sum();
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
    // The upward triangles (carriageway and corner sidewalks) tile the polygon's area; the
    // rest are kerbs and skirts.
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
    let first = set.update(&v1.net, &v1.junctions, &w.terrain, &w.content);
    assert!(first.rebuilt >= 25 + 40, "{first:?}");
    let before = set.piece_count();
    // Nothing changed: nothing rebuilt.
    let idle = set.update(&v1.net, &v1.junctions, &w.terrain, &w.content);
    assert_eq!((idle.rebuilt, idle.removed), (0, 0));
    assert!(idle.dirty_chunks.is_empty());

    // One more street end-to-end beside the lattice: only its neighbourhood changes.
    w.line("basic_2l", (X0 + 500.0, Z0), (X0 + 500.0, Z0 + 90.0));
    let v2 = w.view();
    let edit = set.update(&v2.net, &v2.junctions, &w.terrain, &w.content);
    // Counted, not timed (BUG-034): only the neighbourhood is rebuilt; the time is in
    // `road_perf_30x30_grid_and_one_edit`'s report.
    assert!(edit.rebuilt >= 1 && edit.rebuilt <= 6, "{edit:?}");
    assert!(set.piece_count() > before);
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
    let up = set.update(&v2.net, &v2.junctions, &w.terrain, &w.content);
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
    road_perf();
}

/// BUG-034: the 2 ms edit budget is wall-clock, so it is not in the gate. Run with
/// `cargo test -p cp_render --release road_perf_edit_budget -- --ignored`.
#[test]
#[ignore = "wall-clock budget (2 ms per edit); run alone in release"]
fn road_perf_edit_budget() {
    let ms = road_perf();
    assert!(ms <= 2.0, "edit took {ms} ms");
}

/// Builds the lattice, edits it once and returns the edit's update + merge time, ms.
fn road_perf() -> f64 {
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
    let full = set.update(&v1.net, &v1.junctions, &w.terrain, &w.content);
    let full_ms = t.elapsed().as_secs_f64() * 1000.0;

    // One more street segment attached to the lattice edge.
    w.line(
        "basic_2l",
        (x0 + span, z0 + 50.0),
        (x0 + span + 90.0, z0 + 50.0),
    );
    let v2 = w.view();
    let t = std::time::Instant::now();
    let edit = set.update(&v2.net, &v2.junctions, &w.terrain, &w.content);
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
    // 13: the new street starts exactly on a street of the lattice, within a node's snap radius
    // (BUG-073: a point on a street stays where it is, it is no longer pulled to a through node),
    // so the street it starts on is split there: the two halves, the new street and the junction
    // around the split are the 13 rebuilt pieces (3 old ones removed).
    assert!(edit.rebuilt <= 13, "{edit:?}");
    edit_ms + merge_ms
}

/// After edits, the incrementally maintained set equals one built from scratch.
#[test]
fn incremental_update_matches_a_fresh_build() {
    let mut w = world();
    cross(&mut w, "basic_2l");
    let v = w.view();
    let mut inc = RoadSet::default();
    inc.update(&v.net, &v.junctions, &w.terrain, &w.content);

    // Edits: a branch, an upgrade of a segment, a removal.
    w.line(
        "basic_2l",
        (X0 + 100.0, Z0 + 100.0),
        (X0 + 190.0, Z0 + 190.0),
    );
    let v = w.view();
    inc.update(&v.net, &v.junctions, &w.terrain, &w.content);
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
    inc.update(&v.net, &v.junctions, &w.terrain, &w.content);
    let (seg, _) = v.net.segments().nth(2).unwrap();
    let tick = w.sim.tick();
    w.sim
        .push_command(SimCommand::Net(NetCommand::RemoveSegment { seg }), tick);
    w.sim.tick_once();
    w.sim.tick_once();
    let v = w.view();
    inc.update(&v.net, &v.junctions, &w.terrain, &w.content);

    let mut fresh = RoadSet::default();
    fresh.update(&v.net, &v.junctions, &w.terrain, &w.content);
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

/// BUG-025: the deck of every ground road type is above the terrain (bilinear ground and the
/// triangles of every LOD's mesh) at each deck vertex and each deck triangle edge midpoint.
#[test]
fn ground_roads_stay_above_the_terrain_mesh() {
    const EPS: f32 = 0.05;
    for map in ["flatlands", "river_valley"] {
        let mut w = world_on(map);
        let roads: Vec<String> = w.content.roads().map(|(_, d)| d.id.clone()).collect();
        let mut built = 0;
        for (k, road) in roads.iter().enumerate() {
            // Several lines per type, in different directions, over uneven ground.
            for (i, (dx, dz)) in [(400.0, 0.0), (0.0, 400.0), (280.0, 280.0), (300.0, -150.0)]
                .into_iter()
                .enumerate()
            {
                let x = 1500.0 + 1000.0 * i as f32 + 60.0 * k as f32;
                let z = 3000.0 + 90.0 * k as f32;
                w.line(
                    road,
                    (x, z + 450.0 * (dz < 0.0) as i32 as f32),
                    (x + dx, z + dz + 450.0 * (dz < 0.0) as i32 as f32),
                );
            }
        }
        let v = w.view();
        let (set, _) = w.set(&v);
        let heights = |p: Vec2| {
            (0..crate::chunk::LOD_COUNT)
                .map(|l| lod_height(&w.terrain, l, p))
                .fold(w.terrain.height(p), f32::max)
        };
        let check = |m: &MeshData, what: &str| {
            let pos: Vec<Vec3> = positions(m).collect();
            let up = |i: u32| m.normals[i as usize][1] > 0.9;
            for (i, p) in pos.iter().enumerate() {
                if up(i as u32) {
                    let h = heights(Vec2::new(p.x, p.z));
                    assert!(p.y >= h + EPS, "{map} {what}: vertex {p} below terrain {h}");
                }
            }
            for t in m.indices.chunks(3) {
                if !t.iter().all(|&i| up(i)) {
                    continue;
                }
                for (a, b) in [(0, 1), (1, 2), (2, 0)] {
                    let mid = (pos[t[a] as usize] + pos[t[b] as usize]) * 0.5;
                    let h = heights(Vec2::new(mid.x, mid.z));
                    assert!(
                        mid.y >= h + EPS,
                        "{map} {what}: edge midpoint {mid} below terrain {h}"
                    );
                }
            }
        };
        for (id, seg) in v.net.segments() {
            if seg.elevation != Elevation::Ground {
                continue;
            }
            let piece = set.piece(PieceKey::Segment(id)).unwrap();
            check(&piece.near, "segment near");
            check(&piece.far, "segment far");
            built += 1;
        }
        for (node, _) in v.junctions.iter() {
            if let Some(piece) = set.piece(PieceKey::Junction(*node)) {
                check(&piece.near, "junction");
            }
        }
        assert!(
            built >= roads.len(),
            "{map}: only {built} ground segments built"
        );
    }
}

// ---------------------------------------------------------------------------------------
// BUG-023 / BUG-024: junctions: sidewalks never cover a carriageway, the canvas has no gaps

type Tri2 = [Vec2; 3];

/// Area of the intersection of two triangles in plan (Sutherland-Hodgman).
fn overlap_area(a: Tri2, b: Tri2) -> f32 {
    // Work near the origin: world coordinates are in the thousands and f32 loses the area.
    let o = a[0];
    let (a, b) = (a.map(|p| p - o), b.map(|p| p - o));
    let ccw = |t: Tri2| {
        if (t[1] - t[0]).perp_dot(t[2] - t[0]) < 0.0 {
            [t[0], t[2], t[1]]
        } else {
            t
        }
    };
    let (a, b) = (ccw(a), ccw(b));
    let mut poly: Vec<Vec2> = a.to_vec();
    for i in 0..3 {
        let (p, q) = (b[i], b[(i + 1) % 3]);
        let side = |x: Vec2| (q - p).perp_dot(x - p);
        let input = std::mem::take(&mut poly);
        for k in 0..input.len() {
            let (c, d) = (input[k], input[(k + 1) % input.len()]);
            let (sc, sd) = (side(c), side(d));
            if sc >= 0.0 {
                poly.push(c);
            }
            if (sc >= 0.0) != (sd >= 0.0) {
                poly.push(c + (d - c) * (sc / (sc - sd)));
            }
        }
        if poly.is_empty() {
            return 0.0;
        }
    }
    let n = poly.len();
    (0..n)
        .map(|i| poly[i].perp_dot(poly[(i + 1) % n]))
        .sum::<f32>()
        .abs()
        / 2.0
}

fn in_tri(p: Vec2, t: Tri2) -> bool {
    // Relative to the triangle: world coordinates are in the thousands.
    let (p, t) = (p - t[0], t.map(|v| v - t[0]));
    let s = |a: Vec2, b: Vec2| (b - a).perp_dot(p - a);
    let (d0, d1, d2) = (s(t[0], t[1]), s(t[1], t[2]), s(t[2], t[0]));
    let eps = -0.05; // f32 at 4700 m has a 0.5 mm step
    (d0 >= eps && d1 >= eps && d2 >= eps) || (d0 <= -eps && d1 <= -eps && d2 <= -eps)
}

/// Plan triangles of the upward faces of `m` drawn in `colour`.
fn colour_tris(m: &MeshData, colour: [f32; 4]) -> Vec<Tri2> {
    (0..m.triangle_count())
        .filter(|&t| tri_normal(m, t).y > 0.99)
        .filter(|&t| m.colors[m.indices[t * 3] as usize] == colour)
        .map(|t| {
            [0, 1, 2].map(|k| {
                let p = m.positions[m.indices[t * 3 + k] as usize];
                Vec2::new(p[0], p[2])
            })
        })
        .collect()
}

fn piece_keys(v: &SimView) -> Vec<PieceKey> {
    let mut keys: Vec<PieceKey> = v
        .net
        .segments()
        .map(|(id, _)| PieceKey::Segment(id))
        .collect();
    keys.extend(v.junctions.iter().map(|(n, _)| PieceKey::Junction(*n)));
    keys
}

/// Plan area where a sidewalk triangle lies over asphalt, of a segment or of a node (T-263: the
/// corner sidewalks lie beside the carriageway, not over it).
fn sidewalk_over_segment_asphalt(set: &RoadSet, v: &SimView) -> f32 {
    let (mut sw, mut asph) = (Vec::new(), Vec::new());
    // A middle node has no piece (T-263).
    for p in piece_keys(v).into_iter().filter_map(|k| set.piece(k)) {
        sw.extend(colour_tris(&p.near, SIDEWALK));
        asph.extend(colour_tris(&p.near, ASPHALT));
    }
    sw.iter()
        .flat_map(|s| asph.iter().map(|a| overlap_area(*s, *a)))
        .sum()
}

/// Distance from `p` to segment `a`-`b`.
fn dist_to_edge(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.length_squared().max(1e-9)).clamp(0.0, 1.0);
    p.distance(a + ab * t)
}

fn in_polygon(p: Vec2, poly: &[Vec2]) -> bool {
    let n = poly.len();
    let mut inside = false;
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + 1) % n]);
        if (a.y > p.y) != (b.y > p.y) && p.x < a.x + (p.y - a.y) / (b.y - a.y) * (b.x - a.x) {
            inside = !inside;
        }
    }
    inside
}

const C: Vec2 = Vec2::new(X0 + 100.0, Z0 + 100.0);

fn dir(deg: f32) -> Vec2 {
    Vec2::from_angle(deg.to_radians())
}

fn road(w: &mut World, a: Vec2, b: Vec2) {
    road_of(w, "basic_2l", a, b);
}

fn road_of(w: &mut World, def: &str, a: Vec2, b: Vec2) {
    w.line(def, (a.x, a.y), (b.x, b.y));
}

/// BUG-065: every road def with sidewalks, narrow to wide.
const SIDEWALK_ROADS: [&str; 5] = [
    "basic_2l",
    "basic_1w_2l",
    "medium_1w_3l",
    "medium_4l",
    "large_6l",
];

/// A T: a main road through `C` along `main_deg` and a side road at `side_deg` to it that
/// ends on it (`side_first`: the side road is drawn first/last in the other direction).
fn t_junction(w: &mut World, def: &str, main_deg: f32, side_deg: f32, from_main: bool) {
    road_of(w, def, C - dir(main_deg) * 100.0, C + dir(main_deg) * 100.0);
    let far = C + dir(side_deg) * 90.0;
    if from_main {
        road_of(w, def, C, far);
    } else {
        road_of(w, def, far, C);
    }
}

/// A cross of two main roads and two side roads through `C`.
fn crossing(w: &mut World, def: &str, main_deg: f32, cross_deg: f32) {
    road_of(w, def, C - dir(main_deg) * 100.0, C + dir(main_deg) * 100.0);
    road_of(w, def, C - dir(cross_deg) * 90.0, C + dir(cross_deg) * 90.0);
}

/// Two roads meeting at `C` with an angle of `deg` between them: a node of degree 2.
fn corner(w: &mut World, def: &str, deg: f32, reversed: bool) {
    let a = C - Vec2::X * 90.0;
    let b = C + dir(deg) * 90.0;
    road_of(w, def, a, C);
    if reversed {
        road_of(w, def, b, C);
    } else {
        road_of(w, def, C, b);
    }
}

fn check_clean(w: &World, what: &str) {
    let v = w.view();
    assert_eq!(
        v.junctions.len(),
        v.net.node_count(),
        "{what}: nodes without a junction"
    );
    let (set, _) = w.set(&v);
    let over = sidewalk_over_segment_asphalt(&set, &v);
    assert!(
        over < 0.01,
        "{what}: sidewalk over the carriageway: {over} m2"
    );
    for (node, j) in v.junctions.iter() {
        check_rim(&set, &v.net, j, node, what);
    }
}

/// The piece of node `j` covers its outline (`Junction::polygon`) without holes, a sidewalk
/// lies only along its rim (never further from it than the widest sidewalk), and a junction has
/// no sidewalk at its centre; a middle node has no piece (T-263).
fn check_rim(set: &RoadSet, graph: &NetGraph, j: &Junction, node: &NodeId, what: &str) {
    // The widest sidewalk of the junction's roads (T-275: roads are whole cells wide, the
    // sidewalks take the rest), and a little.
    let widest = j
        .arms
        .iter()
        .filter_map(|a| graph.segment(a.segment))
        .flat_map(|s| s.lanes.iter().filter_map(|&l| graph.lane(l)))
        .filter(|l| l.kind == cp_content::LaneKind::Pedestrian)
        .map(|l| l.width)
        .fold(2.5f32, f32::max)
        + 0.5;
    let piece = set.piece(PieceKey::Junction(*node));
    if j.class == NodeClass::Middle {
        assert!(piece.is_none(), "{what}: a middle node has a piece");
        return;
    }
    let m = &piece
        .unwrap_or_else(|| panic!("{what}: no piece for {node:?}"))
        .near;
    let up: Vec<Tri2> = (0..m.triangle_count())
        .filter(|&t| tri_normal(m, t).y > 0.5)
        .map(|t| {
            let p = |k: usize| {
                let q = m.positions[m.indices[t * 3 + k] as usize];
                Vec2::new(q[0], q[2])
            };
            [p(0), p(1), p(2)]
        })
        .collect();
    let sw = colour_tris(m, SIDEWALK);
    let poly = &j.polygon;
    let n = poly.len();
    // The rim: outline edges that are not an arm's end line, and the corners where two arms'
    // end lines meet (inside corners whose outer curve shrank to the meeting point).
    // An arm's end line: one arm owns both ends (a shared inside corner belongs to two arms).
    let end_line = |i: usize| {
        let on = |a: &cp_net::Arm, p: Vec2| a.right.distance(p) < 1e-3 || a.left.distance(p) < 1e-3;
        j.arms
            .iter()
            .any(|a| on(a, poly[i]) && on(a, poly[(i + 1) % n]))
    };
    let outer: Vec<(Vec2, Vec2)> = (0..n)
        .filter(|&i| !end_line(i))
        .map(|i| (poly[i], poly[(i + 1) % n]))
        .collect();
    let corners: Vec<Vec2> = (0..n)
        .filter(|&i| end_line((i + n - 1) % n) && end_line(i))
        .map(|i| poly[i])
        .collect();
    let (lo, hi) = poly.iter().fold(
        (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN)),
        |(l, h), p| (l.min(*p), h.max(*p)),
    );
    let centre = to_xz(j.pos);
    let mut x = lo.x;
    while x <= hi.x {
        let mut y = lo.y;
        while y <= hi.y {
            let p = Vec2::new(x, y);
            y += 0.7;
            // Points right on an edge can fall between triangles: stay off the boundary.
            if !in_polygon(p, poly)
                || (0..n).any(|i| dist_to_edge(p, poly[i], poly[(i + 1) % n]) < 0.05)
            {
                continue;
            }
            assert!(
                up.iter().any(|t| in_tri(p, *t)),
                "{what}: hole in the node piece at {p:?}"
            );
            let walked = sw.iter().any(|t| in_tri(p, *t));
            assert!(
                !walked || p.distance(centre) > 3.0 || j.arms.len() < 3,
                "{what}: sidewalk on the carriageway at the node centre {p:?}"
            );
            let rim = outer
                .iter()
                .map(|&(a, b)| dist_to_edge(p, a, b))
                .chain(corners.iter().map(|c| p.distance(*c) * 0.2))
                .fold(f32::MAX, f32::min);
            assert!(
                !walked || rim < widest,
                "{what}: sidewalk {rim} m from the rim at {p:?}"
            );
        }
        x += 0.7;
    }
}

/// Convex corners of junction `j` (neighbouring arms less than 150 degrees apart) whose
/// sidewalk is there: a point half a metre in from where the two outer edges meet is pavement.
fn corner_walks(set: &RoadSet, j: &Junction, node: &NodeId) -> usize {
    let m = &set.piece(PieceKey::Junction(*node)).unwrap().near;
    let sw = colour_tris(m, SIDEWALK);
    let k = j.arms.len();
    let centre = to_xz(j.pos);
    (0..k)
        .filter(|&i| {
            let (a, b) = (&j.arms[i], &j.arms[(i + 1) % k]);
            let gap = (b.angle - a.angle).rem_euclid(std::f32::consts::TAU);
            if gap >= cp_net::FLAT_GAP_DEG.to_radians() {
                return false;
            }
            let x = (a.left + b.right) * 0.5;
            let probe = x + (centre - x).normalize_or_zero() * 0.5;
            sw.iter().any(|t| in_tri(probe, *t))
        })
        .count()
}

#[test]
fn t_junctions_have_no_sidewalk_over_the_carriageway() {
    for (main, side, from_main) in [
        (0.0, 90.0, false),
        (0.0, 90.0, true),
        (0.0, -90.0, false),
        (20.0, 110.0, true),
        (20.0, 110.0, false),
        (20.0, 70.0, true),
        (33.0, 100.0, false),
        (0.0, 60.0, false),
        (0.0, 45.0, true),
        (0.0, 135.0, false),
    ] {
        for def in SIDEWALK_ROADS {
            let mut w = world();
            t_junction(&mut w, def, main, side, from_main);
            check_clean(
                &w,
                &format!("{def} T main {main} side {side} from_main {from_main}"),
            );
        }
    }
}

#[test]
fn crossings_have_no_sidewalk_over_the_carriageway() {
    for (a, b) in [(0.0, 90.0), (15.0, 105.0), (0.0, 60.0), (0.0, 45.0)] {
        for def in SIDEWALK_ROADS {
            let mut w = world();
            crossing(&mut w, def, a, b);
            check_clean(&w, &format!("{def} cross {a}/{b}"));
        }
    }
}

#[test]
fn corners_are_one_continuous_canvas_at_90_60_45_and_30_degrees_between_the_roads() {
    for (def, deg, reversed) in SIDEWALK_ROADS.iter().flat_map(|d| {
        [90.0, 120.0, 135.0, 150.0, 60.0, 45.0]
            .into_iter()
            .flat_map(move |deg| [(*d, deg, false), (*d, deg, true)])
    }) {
        {
            let mut w = world();
            corner(&mut w, def, deg, reversed);
            check_clean(&w, &format!("{def} corner {deg} reversed {reversed}"));
            // The canvas covers the gap between the two segment ends.
            let v = w.view();
            let (set, _) = w.set(&v);
            let (node, j) = v
                .junctions
                .iter()
                .find(|(_, j)| j.arms.len() == 2 && j.polygon.len() >= 4)
                .expect("the corner node");
            let m = &set.piece(PieceKey::Junction(*node)).unwrap().near;
            // Lanes and their markings run on through a bend (T-263): every upward face.
            let canvas: Vec<Tri2> = (0..m.triangle_count())
                .filter(|&t| tri_normal(m, t).y > 0.5)
                .map(|t| {
                    let p = |k: usize| {
                        let q = m.positions[m.indices[t * 3 + k] as usize];
                        Vec2::new(q[0], q[2])
                    };
                    [p(0), p(1), p(2)]
                })
                .collect();
            let mid = |a: &Arm| (a.right + a.left) * 0.5;
            let (m0, m1) = (mid(&j.arms[0]), mid(&j.arms[1]));
            for i in 0..=20 {
                let p = m0.lerp(m1, i as f32 / 20.0);
                assert!(
                    canvas.iter().any(|t| in_tri(p, *t)),
                    "{def} corner {deg}: gap at {p:?} between the segment ends"
                );
            }
        }
    }
}

/// BUG-023/024 as the player met them: the game paused, so commands change the graph but no
/// tick runs the junction update. The junctions must still be there and the roads trimmed.
#[test]
fn roads_built_while_paused_are_trimmed_and_have_their_junctions() {
    let mut w = world();
    w.paused = true;
    t_junction(&mut w, "basic_2l", 0.0, 90.0, false);
    check_clean(&w, "paused T");
    let mut w = world();
    w.paused = true;
    corner(&mut w, "basic_2l", 90.0, false);
    check_clean(&w, "paused corner");
}

#[test]
fn a_straight_join_keeps_its_sidewalks_across_the_node() {
    // Two collinear segments (an auto-cut long road): a middle node without a piece (T-263);
    // the segments' sidewalks meet on its end line.
    let mut w = world();
    road(&mut w, C - Vec2::X * 100.0, C + Vec2::X * 100.0);
    check_clean(&w, "long road");
    let v = w.view();
    let (set, _) = w.set(&v);
    let (node, j) = v
        .junctions
        .iter()
        .find(|(_, j)| j.arms.len() == 2)
        .expect("a join");
    assert_eq!(j.class, NodeClass::Middle);
    assert!(set.piece(PieceKey::Junction(*node)).is_none());
    let sw: Vec<Tri2> = j
        .arms
        .iter()
        .flat_map(|a| {
            colour_tris(
                &set.piece(PieceKey::Segment(a.segment)).unwrap().near,
                SIDEWALK,
            )
        })
        .collect();
    let c = to_xz(j.pos);
    // On both sides, just before and after the node, there is pavement.
    for side in [-1.0, 1.0] {
        for along in [-0.3, 0.3] {
            let p = c + Vec2::new(along, side * 5.0);
            assert!(
                sw.iter().any(|t| in_tri(p, *t)),
                "no sidewalk at {along} on side {side}"
            );
        }
    }
}

#[test]
fn a_cross_has_pavement_in_its_four_corners_and_a_t_along_its_far_side() {
    let mut w = world();
    crossing(&mut w, "basic_2l", 0.0, 90.0);
    let v = w.view();
    let (set, _) = w.set(&v);
    let (node, _) = v.junctions.iter().find(|(_, j)| j.arms.len() == 4).unwrap();
    let sw = colour_tris(
        &set.piece(PieceKey::Junction(*node)).unwrap().near,
        SIDEWALK,
    );
    for (sx, sz) in [(-1.0, -1.0), (-1.0, 1.0), (1.0, -1.0), (1.0, 1.0)] {
        // 1 m in from the corner of the 12 m square, along both roads' sidewalks.
        let p = C + Vec2::new(sx * 5.0, sz * 5.0);
        assert!(
            sw.iter().any(|t| in_tri(p, *t)),
            "no pavement at corner {sx},{sz}"
        );
    }
    let mut w = world();
    t_junction(&mut w, "basic_2l", 0.0, 90.0, false);
    let v = w.view();
    let (set, _) = w.set(&v);
    let (node, _) = v.junctions.iter().find(|(_, j)| j.arms.len() == 3).unwrap();
    let sw = colour_tris(
        &set.piece(PieceKey::Junction(*node)).unwrap().near,
        SIDEWALK,
    );
    // The side road runs towards +z, so the main road's far side is z = C.y - 5: it is
    // paved across the mouth's width, and the near side (the mouth) is open.
    assert!(sw.iter().any(|t| in_tri(C + Vec2::new(0.0, -5.0), *t)));
    assert!(!sw.iter().any(|t| in_tri(C + Vec2::new(0.0, 5.0), *t)));
}

/// BUG-054 / BUG-055, T-263: on the outside of a bend the sidewalk is one band around the node
/// (a round join): no mesh vertex lies outside the outline (spikes), and the band is as wide
/// at the middle of the arc as beside it (no notch of asphalt).
#[test]
fn a_bend_has_one_smooth_outer_sidewalk_without_spikes_or_notches() {
    let db = world().content;
    for (def, len) in SIDEWALK_ROADS
        .iter()
        .flat_map(|d| [20.0f32, 30.0, 60.0, 90.0].map(|l| (*d, l)))
    {
        for deg in [90.0f32, 60.0, 45.0, 120.0, 135.0] {
            // Arms shorter than twice the road's width at an obtuse angle overlap each other's
            // road (the segments' own sidewalks, not the node's): out of this test's reach.
            let width = db.road(db.road_id(def).unwrap()).width;
            if deg > 100.0 && len < 2.0 * width {
                continue;
            }
            for reversed in [false, true] {
                let mut w = world();
                let a = C - Vec2::X * len;
                let b = C + dir(deg) * len;
                road_of(&mut w, def, a, C);
                if reversed {
                    road_of(&mut w, def, b, C)
                } else {
                    road_of(&mut w, def, C, b)
                }
                let what = format!("{def} bend {deg} len {len} reversed {reversed}");
                check_clean(&w, &what);
                let v = w.view();
                let (set, _) = w.set(&v);
                let (node, j) = v
                    .junctions
                    .iter()
                    .find(|(_, j)| j.arms.len() == 2)
                    .expect("the bend node");
                // A one-way road drawn head-on is a transition (its lanes do not run on).
                let one_way_head_on = def.contains("1w") && reversed;
                assert_eq!(
                    j.class,
                    if one_way_head_on {
                        NodeClass::Transition
                    } else {
                        NodeClass::Bend
                    },
                    "{what}"
                );
                let rp = &j.polygon;
                let m = &set.piece(PieceKey::Junction(*node)).unwrap().near;
                let n = rp.len();
                for (i, p) in m.positions.iter().enumerate() {
                    let q = Vec2::new(p[0], p[2]);
                    let edge = (0..n)
                        .map(|k| dist_to_edge(q, rp[k], rp[(k + 1) % n]))
                        .fold(f32::MAX, f32::min);
                    assert!(
                        in_polygon(q, rp) || edge < 0.05,
                        "{what}: vertex {i} at {q:?} sticks out of the outline ({edge} m)"
                    );
                }
                // A point 1.2 m in from the middle of the round join is pavement.
                let sw = colour_tris(m, SIDEWALK);
                let centre = to_xz(j.pos);
                let half = j.arms[0].width * 0.5;
                let arc: Vec<Vec2> = rp
                    .iter()
                    .copied()
                    .filter(|p| (p.distance(centre) - half).abs() < 0.01)
                    .collect();
                let at = arc[arc.len() / 2];
                let probe = at + (centre - at).normalize_or_zero() * 1.2;
                assert!(
                    sw.iter().any(|t| in_tri(probe, *t)),
                    "{what}: asphalt notch in the outer sidewalk at {probe:?}"
                );
            }
        }
    }
}

/// T-277 (spec v2 item 10): an L turn is one sharp node, but its inner kerb is rounded: a kerb
/// radius round the block's corner (a point just inside the sidewalk at the old sharp kerb corner
/// is road now), the road edge keeps its sharp corner (so the cells beside it do not change), and
/// the middle of the road is no sidewalk.
#[test]
fn an_l_turn_has_a_rounded_inner_kerb_and_a_sharp_road_edge() {
    for (def, kerb, hw) in [("basic_2l", 3.5f32, 8.0f32), ("medium_4l", 7.0, 16.0)] {
        let mut w = world();
        road_of(&mut w, def, C - Vec2::X * 80.0, C);
        road_of(&mut w, def, C, C + dir(90.0) * 80.0);
        let v = w.view();
        let (set, _) = w.set(&v);
        let (node, j) = v
            .junctions
            .iter()
            .find(|(_, j)| j.arms.len() == 2)
            .expect("the L node");
        assert_eq!(j.class, NodeClass::Bend, "{def}");
        let m = &set.piece(PieceKey::Junction(*node)).unwrap().near;
        let sw = colour_tris(m, SIDEWALK);
        let walked = |p: Vec2| sw.iter().any(|t| in_tri(p, *t));
        // The inner side lies between the arms, towards (-1, 1).
        let inner = Vec2::new(-1.0, 1.0);
        let kerb_corner = C + inner * kerb;
        assert!(
            !walked(kerb_corner + inner * 0.4),
            "{def}: the inner kerb keeps its sharp corner"
        );
        assert!(
            walked(kerb_corner + inner * 2.0),
            "{def}: no sidewalk inside the rounded kerb"
        );
        assert!(
            walked(C + inner * (hw - 0.3)),
            "{def}: the road edge lost its sharp corner"
        );
        assert!(!walked(C), "{def}: sidewalk in the middle of the road");
    }
}

// T-233: street lamps and utility poles.

fn furniture(w: &World) -> (furniture::FurnitureSet, Update) {
    let view = w.view();
    let mut set = furniture::FurnitureSet::default();
    let up = set.update(&view.net, &view.junctions, &w.terrain, &w.content);
    (set, up)
}

#[test]
fn a_street_gets_lamps_on_both_sidewalks_and_poles_on_one_inside_the_end_margins() {
    let mut w = world();
    w.line("basic_2l", (4000.0, 4000.0), (4200.0, 4000.0));
    let (set, up) = furniture(&w);
    let (lamps, poles) = set.counts();
    assert!((10..=16).contains(&lamps), "{lamps} lamps");
    assert!((3..=6).contains(&poles), "{poles} poles");
    assert_eq!(up.dirty_chunks.len(), set.chunks().count());
    // Heads hang over the carriageway, well above the sidewalk, away from both ends.
    let mut heads = 0;
    for c in set.chunks() {
        let m = set.chunk_mesh(*c, 1).unwrap();
        for p in &m.positions {
            heads += 1;
            let ground = w.terrain.height(Vec2::new(p[0], p[2]));
            assert!(
                p[1] - ground > 4.5 && p[1] - ground < 7.5,
                "head height {p:?} over {ground}"
            );
            assert!(p[0] > 4000.0 + 4.0 && p[0] < 4200.0 - 4.0, "{p:?}");
            assert!((p[2] - 4000.0).abs() < 8.0, "over the street: {p:?}");
        }
        // Pools lie on the road, a few centimetres up, and fade to nothing at the rim.
        let pools = set.chunk_mesh(*c, 2).unwrap();
        assert!(pools.positions.iter().all(|p| {
            let ground = w.terrain.height(Vec2::new(p[0], p[2]));
            p[1] - ground < 2.5
        }));
        assert!(pools.colors.iter().any(|c| c[3] == 0.0));
        assert!(pools.colors.iter().any(|c| c[3] > 0.3));
        assert!(set.chunk_mesh(*c, 0).unwrap().triangle_count() > 0);
    }
    assert!(
        heads >= lamps * 24,
        "{heads} head vertices for {lamps} lamps"
    );
    // Nothing changed: nothing rebuilt.
    let view = w.view();
    let mut set = set;
    let again = set.update(&view.net, &view.junctions, &w.terrain, &w.content);
    assert_eq!((again.rebuilt, again.removed), (0, 0));
}

#[test]
fn highways_and_junctions_get_no_lamps_and_a_junction_keeps_them_clear() {
    let mut w = world();
    w.line("highway_1w_2l", (4000.0, 4400.0), (4200.0, 4400.0));
    let (set, _) = furniture(&w);
    assert_eq!(set.counts(), (0, 0), "a highway has none");

    // A T: no lamp inside the junction area (every lamp is 5 m or more from the crossing).
    let mut w = world();
    w.line("basic_2l", (4000.0, 4000.0), (4200.0, 4000.0));
    w.line("basic_2l", (4100.0, 3900.0), (4100.0, 4000.0));
    let (set, _) = furniture(&w);
    for c in set.chunks() {
        for p in &set.chunk_mesh(*c, 1).unwrap().positions {
            let (dx, dz) = ((p[0] - 4100.0).abs(), (p[2] - 4000.0).abs());
            assert!(dx.max(dz) > 6.0, "a lamp head in the junction: {p:?}");
        }
    }
}

// T-222: a path is a flat gravel ribbon, no kerbs.
#[test]
fn a_path_is_flat_gravel_without_kerbs_or_sidewalk() {
    let mut w = world();
    w.line("park_path", (X0, Z0), (X0 + 80.0, Z0));
    w.line("park_path", (X0 + 80.0, Z0), (X0 + 80.0, Z0 + 60.0));
    let v = w.view();
    let (set, _) = w.set(&v);
    let (id, _) = v.net.segments().next().unwrap();
    let piece = set.piece(PieceKey::Segment(id)).unwrap();
    let m = &piece.near;
    assert!(m.colors.contains(&GRAVEL));
    assert!(
        m.colors
            .iter()
            .all(|c| ![SIDEWALK, KERB, ASPHALT].contains(c))
    );
    // One flat level: no kerb faces and no raised band.
    let ys: Vec<f32> = positions(m)
        .zip(&m.colors)
        .filter(|(_, c)| **c == GRAVEL)
        .map(|(p, _)| p.y)
        .collect();
    let (lo, hi) = ys
        .iter()
        .fold((f32::MAX, f32::MIN), |(l, h), &y| (l.min(y), h.max(y)));
    assert!(hi - lo < 0.7, "{lo}..{hi}"); // follows the slope only
    // The corner junction of two paths is gravel too.
    let (n, _) = v.net.nodes().find(|(_, n)| n.segments.len() == 2).unwrap();
    let j = set.piece(PieceKey::Junction(n)).unwrap();
    assert!(j.near.colors.contains(&GRAVEL));
    assert!(
        j.near
            .colors
            .iter()
            .all(|c| ![SIDEWALK, KERB, ASPHALT].contains(c))
    );
}

/// BUG-065: far from the origin (float steps of ~0.0005 m) the corners of neighbouring arms are
/// one point, so every corner of an X and a T gets its pavement patch; the main road is long
/// enough to be auto-cut into joints beside the junction.
#[test]
fn junctions_far_out_on_the_map_get_a_patch_in_every_corner() {
    let d = C + Vec2::new(600.0, 300.0);
    for def in SIDEWALK_ROADS {
        for side in [None, Some(90.0f32)] {
            let mut w = world();
            road_of(&mut w, def, d - Vec2::X * 160.0, d + Vec2::X * 160.0);
            match side {
                None => road_of(&mut w, def, d - Vec2::Y * 120.0, d + Vec2::Y * 120.0),
                Some(deg) => road_of(&mut w, def, d + dir(deg) * 120.0, d),
            }
            let what = format!("{def} {}", if side.is_none() { "X" } else { "T" });
            check_clean(&w, &what);
            let v = w.view();
            let (set, _) = w.set(&v);
            let (node, j) = v
                .junctions
                .iter()
                .find(|(_, j)| j.arms.len() >= 3)
                .unwrap_or_else(|| panic!("{what}: no junction"));
            // The far side of a T is straight: no corner there.
            let corners = if side.is_none() { 4 } else { 2 };
            assert_eq!(
                corner_walks(&set, j, node),
                corners,
                "{what}: a corner without its sidewalk"
            );
        }
    }
}

/// T-240: the zebra stripes of each arm of the junction with `arms` arms: white triangles at
/// least [`ZEBRA_STRIPE`] - 0.1 m wide across the road (lane lines are 0.12 m) on the arm's
/// segment piece. Per arm: (stripe triangles, the mouth's half width, its corners).
fn zebras(w: &World, arms: usize) -> (Vec<Vec<Tri2>>, Junction) {
    let v = w.view();
    let (set, _) = w.set(&v);
    let (_, j) = v
        .junctions
        .iter()
        .find(|(_, j)| j.arms.len() == arms)
        .expect("junction");
    let per_arm = j
        .arms
        .iter()
        .map(|arm| {
            let across = (arm.left - arm.right).normalize();
            let m = &set.piece(PieceKey::Segment(arm.segment)).unwrap().near;
            colour_tris(m, WHITE)
                .into_iter()
                .filter(|t| {
                    let d: Vec<f32> = t.iter().map(|p| p.dot(across)).collect();
                    let span = d.iter().fold(f32::MIN, |a, b| a.max(*b))
                        - d.iter().fold(f32::MAX, |a, b| a.min(*b));
                    span > ZEBRA_STRIPE - 0.1
                })
                .collect()
        })
        .collect();
    (per_arm, j.clone())
}

#[test]
fn zebras_lie_on_every_arm_of_a_t_and_an_x_inside_the_carriageway_and_outside_the_box() {
    for def in SIDEWALK_ROADS {
        for arms in [3usize, 4] {
            // Arms of their own, 60 m each: no auto-cut joint beside the junction (BUG-069).
            let mut w = world();
            for k in 0..arms {
                road_of(&mut w, def, C + dir(k as f32 * 90.0) * 60.0, C);
            }
            let (per_arm, j) = zebras(&w, arms);
            assert_eq!(per_arm.len(), arms);
            let poly = &j.polygon;
            for (arm, tris) in j.arms.iter().zip(&per_arm) {
                let what = format!("{def} {arms} arms, arm at {}", arm.angle.to_degrees());
                // Two triangles a stripe, at least 3 stripes even on the narrowest road.
                assert!(tris.len() >= 6, "{what}: {} stripe triangles", tris.len());
                let mouth = (arm.left + arm.right) * 0.5;
                let across = (arm.left - arm.right).normalize();
                let out = Vec2::new(-across.y, across.x);
                let out = if out.dot(mouth - to_xz(j.pos)) < 0.0 {
                    -out
                } else {
                    out
                };
                // The kerbs: the outer edges of the carriageway (road width less sidewalks).
                let kerb = arm.left.distance(arm.right) * 0.5 - 2.4;
                for p in tris.iter().flatten() {
                    let along = (*p - mouth).dot(out);
                    assert!(
                        (ZEBRA_GAP - 0.01..=ZEBRA_GAP + ZEBRA_DEPTH + 0.01).contains(&along),
                        "{what}: stripe {along} m from the mouth"
                    );
                    assert!(
                        (*p - mouth).dot(across).abs() < kerb,
                        "{what}: stripe off the carriageway at {p:?}"
                    );
                    let inside = in_polygon(*p, poly)
                        && (0..poly.len())
                            .all(|i| dist_to_edge(*p, poly[i], poly[(i + 1) % poly.len()]) > 0.01);
                    assert!(!inside, "{what}: stripe inside the junction at {p:?}");
                }
            }
        }
    }
}

#[test]
fn highways_bends_and_short_pieces_get_no_zebra() {
    let mut w = world();
    for k in 0..3 {
        road_of(&mut w, "highway_1w_2l", C + dir(k as f32 * 90.0) * 60.0, C);
    }
    let (per_arm, _) = zebras(&w, 3);
    assert!(per_arm.iter().all(Vec::is_empty), "zebra on a highway");
    let mut w = world();
    corner(&mut w, "basic_2l", 90.0, false);
    let (per_arm, _) = zebras(&w, 2);
    assert!(per_arm.iter().all(Vec::is_empty), "zebra on a bend");
    assert!(super::zebra_zones(9.0, true, true).is_empty());
    assert_eq!(super::zebra_zones(30.0, true, false).len(), 3);
}

#[test]
fn zebra_stripes_are_whole_and_centred() {
    let s = zebra_stripes(-3.5, 3.5);
    assert_eq!(s.len(), 7);
    assert!((s[0].0 + s[6].1).abs() < 1e-5);
    assert!(
        s.iter()
            .all(|&(a, b)| a >= -3.25 && b <= 3.25 && (b - a - ZEBRA_STRIPE).abs() < 1e-5)
    );
}

/// BUG-069: an X 8 m and a T 16 m from an automatic cut of the main road (cuts every 64 m,
/// crossings on the 8 m lattice) get every corner patch and a zebra on every arm: the cut
/// beside the junction goes.
#[test]
fn a_junction_beside_an_automatic_cut_gets_its_corners_and_zebras() {
    let d = C + Vec2::new(613.37, 311.13);
    for def in SIDEWALK_ROADS {
        for (arms, from_cut) in [(4usize, 8.0f32), (3, 16.0)] {
            let mut w = world();
            // The main road starts 128 + `from_cut` m before `d`: a cut lies `from_cut` before it.
            road_of(
                &mut w,
                def,
                d - Vec2::X * (128.0 + from_cut),
                d + Vec2::X * 180.0,
            );
            if arms == 4 {
                road_of(&mut w, def, d - Vec2::Y * 120.0, d + Vec2::Y * 120.0);
            } else {
                road_of(&mut w, def, d + Vec2::Y * 120.0, d);
            }
            let what = format!("{def} {arms} arms {from_cut} m from a cut");
            check_clean(&w, &what);
            let (per_arm, j) = zebras(&w, arms);
            let v = w.view();
            let (set, _) = w.set(&v);
            assert_eq!(
                corner_walks(&set, &j, &j.node),
                if arms == 4 { 4 } else { 2 },
                "{what}: a corner without its sidewalk"
            );
            for (arm, tris) in j.arms.iter().zip(&per_arm) {
                assert!(
                    tris.len() >= 6,
                    "{what}: no zebra on the arm at {}",
                    arm.angle.to_degrees()
                );
            }
        }
    }
}

// ---- T-276: parking bays and parked cars --------------------------------------------------

/// The lateral offset of `p` from the trimmed curve of `id` and its arc length there.
fn lateral(v: &SimView, id: SegmentId, p: Vec2) -> (f32, f32) {
    let curve = v.junctions.trimmed_curve(&v.net, id).unwrap();
    let (t, _) = curve.nearest_xz(p);
    let tangent = to_xz(curve.tangent(t)).normalize();
    let left = Vec2::new(tangent.y, -tangent.x);
    (
        (p - to_xz(curve.point(t))).dot(left),
        curve.arc_table().t_to_s(t),
    )
}

#[test]
fn bays_line_local_streets_a_cell_clear_of_junctions_and_not_wide_roads() {
    let mut w = world();
    cross(&mut w, "basic_2l");
    let v = w.view();
    let mut seen = 0;
    for (id, seg) in v.net.segments() {
        let b = parking::bays(&v.net, &v.junctions, id).expect("a local street has bays");
        assert_eq!(b.sides.len(), 2, "both sidewalks");
        let length = v
            .junctions
            .trimmed_curve(&v.net, id)
            .unwrap()
            .arc_table()
            .length();
        for (node, at) in [(seg.start, b.a), (seg.end, length - b.b)] {
            if v.junctions.get(node).is_some_and(|j| j.arms.len() >= 3) {
                assert!(at >= 8.0 - 1e-3, "{at} m from the junction");
            }
        }
        seen += 1;
    }
    assert!(seen >= 4, "{seen} segments (the long arms are cut)");
    for road in ["large_6l", "highway_1w_2l"] {
        let mut w = world();
        w.line(road, (X0, Z0), (X0 + 200.0, Z0));
        let v = w.view();
        assert!(
            v.net
                .segments()
                .all(|(id, _)| parking::bays(&v.net, &v.junctions, id).is_none()),
            "{road}: no bays"
        );
    }
}

#[test]
fn lamps_and_poles_stand_beside_the_bays_not_in_them() {
    let mut w = world();
    cross(&mut w, "basic_2l");
    let v = w.view();
    for (id, _) in v.net.segments() {
        let b = parking::bays(&v.net, &v.junctions, id).unwrap();
        let f =
            furniture::build_segment_furniture(&v.net, &v.junctions, &w.terrain, &w.content, id)
                .expect("lamps");
        let ground = f
            .posts
            .positions
            .iter()
            .map(|p| p[1])
            .fold(f32::MAX, f32::min);
        for p in &f.posts.positions {
            if p[1] > ground + 1.0 {
                continue; // arms and heads reach over the street
            }
            let (x, _) = lateral(&v, id, Vec2::new(p[0], p[2]));
            for side in &b.sides {
                let (lo, hi) = side.range();
                assert!(
                    x < lo + 0.01 || x > hi - 0.01,
                    "a post foot at {x} in the bay {lo}..{hi}"
                );
            }
        }
    }
}

/// A building at `along` metres of segment `id`, 20 m to its left (`side` 1) or right (-1).
#[allow(clippy::too_many_arguments)]
fn front(
    v: &SimView,
    db: &ContentDb,
    grid: &mut cp_building::BuildingGrid,
    def: &str,
    id: SegmentId,
    t: f32,
    side: f32,
    level: u8,
) -> cp_building::BuildingId {
    let seg = v.net.segment(id).unwrap();
    let at = to_xz(seg.curve.point(t));
    let tangent = to_xz(seg.curve.tangent(t)).normalize();
    let left = Vec2::new(tangent.y, -tangent.x);
    grid.insert(cp_building::Building {
        def: db.building_id(def).unwrap(),
        footprint: cp_building::Footprint::Free { w: 2, d: 2 },
        level,
        state: cp_building::BuildingState::Active,
        ready_at: Tick(0),
        pos: at + left * (20.0 * side),
        rot: 0.0,
        access: Some(cp_building::AccessPoint { segment: id, t }),
        flags: cp_building::BuildingFlags::default(),
        level_progress: 0.0,
    })
}

#[test]
fn parked_cars_follow_the_buildings_keep_to_the_bays_and_leave_services_free() {
    let mut w = world();
    w.line("basic_2l", (X0, Z0), (X0 + 96.0, Z0));
    let v = w.view();
    let db = &*w.content;
    let id = v.net.segments().next().unwrap().0;
    let empty = cp_building::BuildingGrid::new();
    let (cars, _) = crate::parked::parked_cars(&v.net, &v.junctions, db, &empty);
    assert!(cars.is_empty(), "empty lots: no parked cars");

    // Busy houses along the left side, a fire station in their middle.
    let mut grid = cp_building::BuildingGrid::new();
    for k in 1..10 {
        front(&v, db, &mut grid, "apt_high_3", id, k as f32 / 10.0, 1.0, 3);
    }
    let station = 0.5;
    front(&v, db, &mut grid, "fire_station", id, station, 1.0, 1);
    let (cars, index) = crate::parked::parked_cars(&v.net, &v.junctions, db, &grid);
    assert!(cars.len() >= 4, "{} cars by busy houses", cars.len());
    let b = parking::bays(&v.net, &v.junctions, id).unwrap();
    let side = b.sides.iter().find(|s| s.sign > 0.0).unwrap();
    let (lo, hi) = side.range();
    let st = lateral(
        &v,
        id,
        to_xz(v.net.segment(id).unwrap().curve.point(station)),
    )
    .1;
    for c in &cars {
        let (x, s) = lateral(&v, id, Vec2::new(c.pos.x, c.pos.z));
        assert!(x > lo && x < hi, "a car at {x}, the left bay is {lo}..{hi}");
        assert!(s > b.a && s < b.b, "a car at {s} outside the bay's stretch");
        assert!(
            (s - st).abs() >= parking::SLOT_LEN - 1e-3,
            "a car before the fire station"
        );
    }
    // The same city, the same cars; a pedestrian walking the bay is drawn beyond it.
    assert_eq!(
        crate::parked::parked_cars(&v.net, &v.junctions, db, &grid).0,
        cars
    );
    let c = cars[0];
    let p = index.nudge(Vec2::new(c.pos.x, c.pos.z));
    let (x, _) = lateral(&v, id, p);
    assert!(x > hi, "nudged to {x}, beyond the bay's edge {hi}");
    let far = Vec2::new(X0 + 48.0, Z0 + 7.5);
    assert_eq!(index.nudge(far), far, "the outer sidewalk is left alone");
}

/// T-302d: the top area of the triangles of colour `c` whose first vertex has `x0 < x < x1`.
fn top_area_in(m: &MeshData, c: [f32; 4], x0: f32, x1: f32, road_z: Option<f32>) -> f32 {
    (0..m.triangle_count())
        .filter(|&t| (0..3).all(|k| m.colors[m.indices[t * 3 + k] as usize] == c))
        .filter(|&t| {
            let p = m.positions[m.indices[t * 3] as usize];
            let z = (0..3)
                .map(|k| m.positions[m.indices[t * 3 + k] as usize][2])
                .sum::<f32>()
                / 3.0;
            // With `road_z`: not the 0.12 m kerb lines at the edge of a road along +x.
            p[0] > x0 && p[0] < x1 && road_z.is_none_or(|rz| (z - rz).abs() < 3.5)
        })
        .map(|t| {
            let p = |k: usize| Vec3::from_array(m.positions[m.indices[t * 3 + k] as usize]);
            (p(1) - p(0)).cross(p(2) - p(0)).length() * 0.5
        })
        .sum()
}

#[test]
fn bus_lanes_carry_the_word_bus_in_the_near_mesh_only_and_lose_no_area() {
    let mut w = world();
    w.line("transitway_2l", (X0, Z0), (X0 + 100.0, Z0));
    let v = w.view();
    let (set, _) = w.set(&v);
    let (id, _) = v.net.segments().next().unwrap();
    let piece = set.piece(PieceKey::Segment(id)).unwrap();
    let len = segment_length(&w, &v, id);
    // "BUS" lights 20 + 15 + 15 pixels of 0.5 x 0.6 m: 15 m2 a word, two words on each of the
    // two lanes of a 100 m segment.
    let words = 2.0 * (len / 40.0).floor();
    // The words: only the paint between their first and last row (the 0.12 m kerb lines and
    // the zebras at the ends are white too).
    let curve = v.junctions.trimmed_curve(&v.net, id).unwrap();
    let words_at = decal::plan(
        &[
            (LaneKind::Bus, LaneDir::Forward, -1.75, 3.5),
            (LaneKind::Bus, LaneDir::Backward, 1.75, 3.5),
        ],
        len,
        &[],
    );
    let (s0, s1) = (words_at[0].s0, words_at[0].s0 + 23.0 * words_at[0].pitch);
    let (x0, x1) = (curve.p0.x + s0 - 0.01, curve.p0.x + s1 - 0.01);
    let white = top_area_in(&piece.near, WHITE, x0, x1, Some(Z0));
    assert!(
        (white - 15.0 * words).abs() < 0.5,
        "{white} vs {}",
        15.0 * words
    );
    assert_eq!(top_area_in(&piece.far, WHITE, x0, x1, Some(Z0)), 0.0);
    // Under the words the lanes (7 m with the yellow line) are covered as without them.
    let in_window = |c| top_area_in(&piece.near, c, x0, x1, Some(Z0));
    let area = in_window(BUS) + in_window(WHITE) + in_window(YELLOW);
    assert!(
        (area - 7.0 * (s1 - s0)).abs() < 0.5,
        "{area} vs {}",
        7.0 * (s1 - s0)
    );
}

#[test]
fn trolley_lanes_carry_trolley_and_tram_lanes_no_word() {
    let mut w = world();
    w.line("trolley_4l", (X0, Z0), (X0 + 100.0, Z0));
    w.line("tram_4l", (X0, Z0 + 100.0), (X0 + 100.0, Z0 + 100.0));
    let v = w.view();
    let (set, _) = w.set(&v);
    let (mut trolley, mut tram, mut steel) = (0.0, 0.0, 0.0);
    for (id, seg) in v.net.segments() {
        let near = &set.piece(PieceKey::Segment(id)).unwrap().near;
        let name = w.content.road(seg.road).id.clone();
        let z = if name == "tram_4l" { Z0 + 100.0 } else { Z0 };
        // Clear of the zebras at the ends of the segment.
        let curve = v.junctions.trimmed_curve(&v.net, id).unwrap();
        let (a, b) = (curve.p0.x.min(curve.p3.x), curve.p0.x.max(curve.p3.x));
        let white = top_area_in(near, WHITE, a + 6.0, b - 6.0, Some(z));
        match name.as_str() {
            "trolley_4l" => trolley += white,
            "tram_4l" => {
                tram += white;
                steel += top_area_in(near, STEEL, a + 6.0, b - 6.0, Some(z));
            }
            other => panic!("{other}"),
        }
    }
    // 'TROLLEY' on both middle lanes of the long segment; the tram lanes have no word.
    assert!(trolley > 10.0, "{trolley}");
    assert!(tram < 1.0, "{tram}");
    // Two rails of 0.14 m on each of the two tram lanes: about 0.56 m2 a metre of the road, in the near
    // mesh only.
    assert!(steel > 10.0, "{steel}");
}

/// BUG-118: a sidewalk that ends at a transition ends in a rounded nose: full width where it
/// starts, never wider than the wedge it replaces plus a small bulge, nothing at the end where
/// the other road has no sidewalk, and the same whichever end it is.
#[test]
fn a_sidewalk_ending_at_a_transition_has_a_rounded_nose() {
    let n = 21;
    let kerb: Vec<Vec2> = (0..n).map(|i| Vec2::new(i as f32, 0.0)).collect();
    let wedge = |i: usize| 9.0 * (1.0 - i as f32 / (n - 1) as f32);
    let outer: Vec<Vec2> = (0..n).map(|i| Vec2::new(i as f32, wedge(i))).collect();
    let width = |p: &[Vec<Vec2>], i: usize| p[0][i].distance(p[1][i]);
    let mut pts = vec![outer.clone(), kerb.clone()];
    node::round_nose(&mut pts, 0, 1, 9.0, 0.0);
    assert!((width(&pts, 0) - 9.0).abs() < 1e-4, "full at the start");
    assert!(width(&pts, n - 1) < 1e-4, "nothing at the end");
    for i in 0..n {
        assert!(width(&pts, i) <= 9.0 + 1e-4);
        // A bulge over the wedge, but a small one.
        assert!(width(&pts, i) <= wedge(i) + 2.5, "point {i}");
    }
    // The nose is blunt: the width drops faster near the end than the wedge does.
    assert!(width(&pts, n - 2) > wedge(n - 2));
    assert!(
        width(&pts, 4) - wedge(4) < 1e-4,
        "the first part is the wedge"
    );
    // Mirrored: the sidewalk grows from nothing at the start.
    let mut rev = vec![
        outer.iter().rev().copied().collect::<Vec<_>>(),
        kerb.iter().rev().copied().collect::<Vec<_>>(),
    ];
    node::round_nose(&mut rev, 0, 1, 0.0, 9.0);
    for i in 0..n {
        assert!((width(&rev, i) - width(&pts, n - 1 - i)).abs() < 1e-4);
    }
    // Sidewalks at both ends (or none) stay as they are.
    let mut same = vec![outer.clone(), kerb.clone()];
    node::round_nose(&mut same, 0, 1, 9.0, 4.0);
    assert_eq!(same[0], outer);
}
