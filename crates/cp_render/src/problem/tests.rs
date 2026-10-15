use bevy_math::{Vec2, Vec3};
use cp_building::{Building, BuildingFlags, BuildingId, BuildingState, Footprint, Lot};
use cp_content::BuildingDefId;
use cp_core::{Id, Tick};

use super::*;

fn collect_now(marked: &[Marked], camera: Vec3, t: f32) -> Vec<IconInstance> {
    collect(marked, &mut Lod::default(), camera, t)
}

fn id(i: u32) -> BuildingId {
    Id::new(i, 0)
}

fn building(state: BuildingState, flags: u8) -> Building {
    Building {
        def: BuildingDefId(0),
        footprint: Footprint::Lot(Lot {
            block: Id::new(0, 0),
            col: 0,
            w: 2,
            d: 2,
        }),
        level: 1,
        state,
        ready_at: Tick(0),
        pos: Vec2::ZERO,
        rot: 0.0,
        access: None,
        flags: BuildingFlags(flags),
        level_progress: 0.0,
    }
}

fn marked_one(flags: u8, state: BuildingState) -> Vec<Marked> {
    let b = building(state, flags);
    mark(std::iter::once((id(1), &b)), |_| {
        Some(Vec3::new(100.0, 12.0, 100.0))
    })
}

#[test]
fn one_flag_gives_one_icon_of_its_kind_over_the_roof() {
    let cases = [
        (BuildingFlags::NO_POWER, Problem::NoPower),
        (BuildingFlags::NO_WATER, Problem::NoWater),
        (BuildingFlags::NO_SEWAGE, Problem::NoWater),
        (BuildingFlags::NO_ROAD, Problem::NoRoad),
        (BuildingFlags::NO_PATH, Problem::NoRoad),
    ];
    for (flag, want) in cases {
        let marked = marked_one(flag, BuildingState::Active);
        let icons = collect_now(&marked, Vec3::new(100.0, 50.0, 100.0), 0.3);
        assert_eq!(icons.len(), 1, "flag {flag}");
        assert_eq!(icons[0].problem, want);
        assert!(icons[0].pos.y >= 12.0 + LIFT_M - BOB_AMPLITUDE_M - 1e-4);
        assert!(icons[0].pos.y <= 12.0 + LIFT_M + BOB_AMPLITUDE_M + 1e-4);
        assert_eq!((icons[0].pos.x, icons[0].pos.z), (100.0, 100.0));
    }
}

#[test]
fn abandoned_building_gets_its_own_icon_after_the_flags() {
    let marked = marked_one(0, BuildingState::Abandoned);
    let icons = collect_now(&marked, Vec3::new(100.0, 50.0, 100.0), 0.0);
    assert_eq!(icons[0].problem, Problem::Abandoned);
    let marked = marked_one(BuildingFlags::NO_POWER, BuildingState::Abandoned);
    assert_eq!(marked[0].problems.len(), 2);
    assert_eq!(marked[0].problems.shown(0.5), Some(Problem::NoPower));
    assert_eq!(marked[0].problems.shown(1.5), Some(Problem::Abandoned));
}

#[test]
fn several_flags_take_turns_each_second_by_importance() {
    let all = BuildingFlags::NO_POWER
        | BuildingFlags::NO_WATER
        | BuildingFlags::NO_ROAD
        | BuildingFlags::NO_SEWAGE;
    let marked = marked_one(all, BuildingState::Active);
    let cam = Vec3::new(100.0, 50.0, 100.0);
    let at = |t: f32| collect_now(&marked, cam, t)[0].problem;
    assert_eq!(at(0.0), Problem::NoRoad);
    assert_eq!(at(0.99), Problem::NoRoad);
    assert_eq!(at(1.0), Problem::NoPower);
    assert_eq!(at(1.99), Problem::NoPower);
    assert_eq!(at(2.0), Problem::NoWater);
    // Back to the first after the last.
    assert_eq!(at(3.0), Problem::NoRoad);
    assert_eq!(at(7.5), Problem::NoPower);
}

#[test]
fn no_flags_no_icon_and_no_icons_while_building_or_burned() {
    assert!(marked_one(0, BuildingState::Active).is_empty());
    let all = 0xFF;
    assert!(marked_one(all, BuildingState::UnderConstruction).is_empty());
    assert!(marked_one(all, BuildingState::Burned).is_empty());
    // Flags that are not problems shown in M1 (no workers, no customers, no goods).
    let other = BuildingFlags::NO_WORKERS | BuildingFlags::NO_CUSTOMERS | BuildingFlags::NO_GOODS;
    assert!(marked_one(other, BuildingState::Active).is_empty());
}

#[test]
fn bob_stays_within_the_amplitude_and_phases_differ() {
    let mut phases = std::collections::BTreeSet::new();
    for i in 0..200 {
        phases.insert((bob_phase(id(i)) * 1000.0) as i32);
        for k in 0..40 {
            let b = bob(id(i), k as f32 * 0.037);
            assert!(b.abs() <= BOB_AMPLITUDE_M + 1e-5);
        }
    }
    assert!(phases.len() > 150, "phases hardly differ: {}", phases.len());
    // One hertz: the same height a second later.
    assert!((bob(id(3), 0.2) - bob(id(3), 1.2)).abs() < 1e-4);
}

/// `n` buildings on a 20 m lattice (20 per row) with the given problems, origin at (1000, 1000).
fn lattice(n: u32, problems: impl Fn(u32) -> Problems) -> Vec<Marked> {
    (0..n)
        .map(|i| Marked {
            id: id(i),
            problems: problems(i),
            top: Vec3::new(
                1000.0 + (i % 20) as f32 * 20.0,
                10.0,
                1000.0 + (i / 20) as f32 * 20.0,
            ),
        })
        .collect()
}

#[test]
fn near_every_building_has_its_own_icon_and_far_they_merge_into_a_few_clusters() {
    let marked = lattice(200, |_| Problems(Problem::NoPower.bit()));
    // 150 m from the lattice: all nearer than NEAR_M.
    let near = collect_now(&marked, Vec3::new(1200.0, 150.0, 1100.0), 0.0);
    assert_eq!(near.len(), 200);
    assert!(near.iter().all(|i| i.count == 1));
    // From 2.5 km away: a handful of clusters whose counts add up to all of them.
    for cam in [
        Vec3::new(1200.0, 500.0, 3600.0),
        Vec3::new(-1300.0, 800.0, 1100.0),
        Vec3::new(1200.0, 3500.0, 1100.0),
    ] {
        let far = collect_now(&marked, cam, 0.0);
        assert!(far.len() <= 10, "{} clusters from {cam}", far.len());
        assert_eq!(far.iter().map(|i| i.count).sum::<u32>(), 200);
        assert!(far.iter().all(|i| i.problem == Problem::NoPower));
    }
    // Nothing is cut off with distance any more.
    let very_far = collect_now(&marked, Vec3::new(1200.0, 9000.0, 1100.0), 0.0);
    assert!(!very_far.is_empty() && very_far.len() <= 4);
}

#[test]
fn the_most_important_problem_wins_a_mixed_cell_and_the_badge_counts_its_buildings() {
    // 30 buildings in one cell far away: 20 without power, 7 without water, 3 without road.
    let marked: Vec<Marked> = (0..30u32)
        .map(|i| Marked {
            id: id(i),
            problems: Problems(match i {
                0..=19 => Problem::NoPower.bit(),
                20..=26 => Problem::NoWater.bit(),
                _ => Problem::NoRoad.bit(),
            }),
            top: Vec3::new(5000.0 + i as f32, 8.0, 5000.0),
        })
        .collect();
    let cam = Vec3::new(5000.0, 2000.0, 5000.0 - 1500.0);
    let at = |t: f32| collect_now(&marked, cam, t);
    assert_eq!(at(0.0).len(), 1);
    assert_eq!((at(0.0)[0].problem, at(0.0)[0].count), (Problem::NoRoad, 3));
    // Then the others, one per second, each with its own count.
    assert_eq!(
        (at(1.0)[0].problem, at(1.0)[0].count),
        (Problem::NoPower, 20)
    );
    assert_eq!(
        (at(2.0)[0].problem, at(2.0)[0].count),
        (Problem::NoWater, 7)
    );
    assert_eq!(at(3.0)[0].problem, Problem::NoRoad);
    // The cluster sits at the centroid of the cell's buildings.
    let c = at(0.0)[0].pos;
    assert!((c.x - 5014.5).abs() < 1e-3 && (c.z - 5000.0).abs() < 1e-3);
    // One building with two problems counts in both.
    let one = lattice(1, |_| {
        Problems(Problem::NoPower.bit() | Problem::NoWater.bit())
    });
    let far = Vec3::new(1000.0, 2500.0, 1000.0);
    assert_eq!(collect_now(&one, far, 0.0)[0].count, 1);
}

#[test]
fn clusters_stay_put_while_the_camera_moves_a_little() {
    let marked = lattice(200, |_| Problems(Problem::NoWater.bit()));
    let keys = |cam: Vec3| {
        let mut v: Vec<_> = collect_now(&marked, cam, 0.0)
            .iter()
            .map(|i| ((i.pos.x * 10.0) as i32, (i.pos.z * 10.0) as i32, i.count))
            .collect();
        v.sort();
        v
    };
    let a = keys(Vec3::new(1200.0, 1500.0, 2900.0));
    assert_eq!(a, keys(Vec3::new(1215.0, 1500.0, 2890.0)));
    assert_eq!(a, keys(Vec3::new(1190.0, 1510.0, 2910.0)));
}

#[test]
fn bands_change_with_hysteresis() {
    assert_eq!(band(390.0, u8::MAX), 0);
    assert_eq!(band(410.0, u8::MAX), 1);
    // Just past the edge a building stays in its band, well past it changes.
    assert_eq!(band(410.0, 0), 0);
    assert_eq!(band(450.0, 0), 1);
    assert_eq!(band(390.0, 1), 1);
    assert_eq!(band(350.0, 1), 0);
    // Jumping several bands at once is allowed.
    assert_eq!(band(5000.0, 0), 4);
}

#[test]
fn the_badge_draws_the_count_and_clusters_cost_more_triangles_than_singles() {
    let facing = Facing {
        position: Vec3::new(0.0, 100.0, 0.0),
        right: Vec3::X,
        up: Vec3::NEG_Z,
    };
    let at = |count| {
        let i = IconInstance {
            id: id(1),
            problem: Problem::NoPower,
            pos: Vec3::ZERO,
            count,
        };
        build_mesh(&[i], &facing)
    };
    assert!(at(37).triangle_count() > at(1).triangle_count());
    // More digits, more segments; a count of 8 lights every segment of its digit.
    assert!(at(88).triangle_count() > at(8).triangle_count());
    for d in 0..10 {
        assert!(!digit_rects(d).is_empty());
    }
    assert_eq!(digit_rects(8).len(), 7);
    assert_eq!(digit_rects(1).len(), 2);
    for (pts, _) in badge(9999) {
        assert!(pts.iter().all(|p| p[0] <= 0.5 + 1e-4 && p[0] >= -0.6));
    }
}

#[test]
fn glyphs_wind_counter_clockwise_and_stay_inside_the_plate() {
    for p in Problem::ALL {
        let (shapes, _) = glyph(p);
        for (pts, _) in shapes {
            let mut area = 0.0;
            for i in 0..pts.len() {
                let (a, b) = (pts[i], pts[(i + 1) % pts.len()]);
                area += a[0] * b[1] - b[0] * a[1];
                assert!(a[0].abs() <= 0.5 && a[1].abs() <= 0.5);
            }
            assert!(area > 0.0, "{p:?} has a clockwise shape");
        }
    }
}

#[test]
fn mesh_has_the_icons_facing_the_camera_and_valid_indices() {
    let facing = Facing {
        position: Vec3::new(0.0, 60.0, 0.0),
        right: Vec3::X,
        up: Vec3::NEG_Z,
    };
    let icons: Vec<_> = Problem::ALL
        .iter()
        .enumerate()
        .map(|(i, p)| IconInstance {
            id: id(i as u32),
            problem: *p,
            pos: Vec3::new(i as f32 * 10.0, 10.0, 5.0),
            count: 1 + i as u32 * 40,
        })
        .collect();
    let m = build_mesh(&icons, &facing);
    assert!(m.indices.iter().all(|&i| (i as usize) < m.positions.len()));
    assert_eq!(m.positions.len(), m.colors.len());
    assert_eq!(m.positions.len(), m.normals.len());
    // Every triangle winds towards the camera (counter-clockwise seen from it).
    for t in m.indices.chunks(3) {
        let v: Vec<Vec3> = t
            .iter()
            .map(|&i| Vec3::from(m.positions[i as usize]))
            .collect();
        let n = (v[1] - v[0]).cross(v[2] - v[0]);
        assert!(n.dot(facing.position - v[0]) > 0.0);
    }
}

#[test]
fn ten_thousand_flagged_buildings_fit_the_render_prep_budget() {
    // (spacing m, camera): all of them near (10k single icons, the worst case for the mesh),
    // and a 1.2 km city seen from its middle (singles and clusters mixed).
    for (spacing, cam, all_single) in [
        (3.0, Vec3::new(150.0, 100.0, 150.0), true),
        (12.0, Vec3::new(600.0, 300.0, 600.0), false),
    ] {
        let marked: Vec<Marked> = (0..10_000u32)
            .map(|i| Marked {
                id: id(i),
                problems: Problems(0b0111),
                top: Vec3::new((i % 100) as f32 * spacing, 15.0, (i / 100) as f32 * spacing),
            })
            .collect();
        let facing = Facing {
            position: cam,
            right: Vec3::X,
            up: Vec3::Z,
        };
        let mut lod = Lod::default();
        let mut best = f32::MAX;
        let mut drawn = 0;
        for frame in 0..5 {
            let start = std::time::Instant::now();
            let icons = collect(&marked, &mut lod, cam, frame as f32 * 0.016);
            let mesh = build_mesh(&icons, &facing);
            best = best.min(start.elapsed().as_secs_f32() * 1000.0);
            drawn = icons.len();
            assert!(mesh.triangle_count() > 0);
        }
        eprintln!("problem icons: spacing {spacing}: {drawn} icons, {best:.2} ms (collect + mesh)");
        if all_single {
            assert_eq!(drawn, 10_000);
        } else {
            assert!(drawn < 10_000);
        }
        // §14 gives render preparation 3 ms in all (this runs at the test profile's opt-level 1).
        assert!(best < 3.0, "{best} ms");
    }
}

/// Draws the four icons and four clusters side by side from the real mesh (camera straight above, painter's
/// order = index order) and checks each shows its plate colour and a glyph. With
/// `T125_RASTER=<path>` set it also writes the picture there (reports/t-125/icons.png).
#[test]
fn the_icons_and_their_clusters_rasterise_square_and_distinct() {
    const CELL: usize = 128;
    let facing = Facing {
        position: Vec3::new(0.0, 100.0, 0.0),
        right: Vec3::X,
        up: Vec3::NEG_Z,
    };
    // Four single icons, then the same four as clusters with a count badge.
    let n = Problem::ALL.len();
    let counts = [1, 1, 1, 1, 37, 128, 1204, 9999];
    let total = counts.len();
    let size = icon_size(100.0);
    // The icon fills about 90 of the cell's 128 pixels.
    let scale = 90.0 / size;
    let spacing = CELL as f32 / scale;
    let icons: Vec<_> = counts
        .iter()
        .enumerate()
        .map(|(i, &count)| IconInstance {
            id: id(i as u32),
            problem: Problem::ALL[i % n],
            pos: Vec3::new(i as f32 * spacing, 0.0, 0.0),
            count,
        })
        .collect();
    let m = build_mesh(&icons, &facing);
    let (w, h) = (CELL * total, CELL);
    let mut img = vec![[236u8, 238, 240]; w * h];
    let px = |v: [f32; 3]| {
        (
            (v[0] + spacing / 2.0) * scale,
            (v[2] + spacing / 2.0) * scale,
        )
    };
    for t in m.indices.chunks(3) {
        let q: Vec<(f32, f32)> = t.iter().map(|&i| px(m.positions[i as usize])).collect();
        let c = m.colors[t[0] as usize];
        let col = [0, 1, 2].map(|k| (c[k].powf(1.0 / 2.2) * 255.0) as u8);
        let (x0, x1) = (
            q.iter()
                .map(|p| p.0)
                .fold(f32::MAX, f32::min)
                .floor()
                .max(0.0) as usize,
            q.iter()
                .map(|p| p.0)
                .fold(f32::MIN, f32::max)
                .ceil()
                .min(w as f32) as usize,
        );
        let (y0, y1) = (
            q.iter()
                .map(|p| p.1)
                .fold(f32::MAX, f32::min)
                .floor()
                .max(0.0) as usize,
            q.iter()
                .map(|p| p.1)
                .fold(f32::MIN, f32::max)
                .ceil()
                .min(h as f32) as usize,
        );
        let edge = |a: (f32, f32), b: (f32, f32), p: (f32, f32)| {
            (b.0 - a.0) * (p.1 - a.1) - (b.1 - a.1) * (p.0 - a.0)
        };
        for y in y0..y1 {
            for x in x0..x1 {
                let p = (x as f32 + 0.5, y as f32 + 0.5);
                let (e0, e1, e2) = (
                    edge(q[0], q[1], p),
                    edge(q[1], q[2], p),
                    edge(q[2], q[0], p),
                );
                if (e0 >= 0.0 && e1 >= 0.0 && e2 >= 0.0) || (e0 <= 0.0 && e1 <= 0.0 && e2 <= 0.0) {
                    img[y * w + x] = col;
                }
            }
        }
    }
    // Square: the dark border spans the same pixels in height and width for every icon.
    let icon_px = (size * scale) as usize;
    let mut sigs = std::collections::BTreeSet::new();
    for i in 0..total {
        let cell: Vec<_> = (0..h)
            .flat_map(|y| (0..CELL).map(move |x| (x, y)))
            .map(|(x, y)| img[y * w + i * CELL + x])
            .collect();
        let dark = |c: &[u8; 3]| c.iter().all(|&v| v < 100);
        let (mut xs, mut ys) = ((usize::MAX, 0), (usize::MAX, 0));
        for (k, c) in cell.iter().enumerate() {
            if dark(c) {
                let (x, y) = (k % CELL, k / CELL);
                xs = (xs.0.min(x), xs.1.max(x));
                ys = (ys.0.min(y), ys.1.max(y));
            }
        }
        if i >= n {
            // A cluster's dark box hangs below the plate: only its distinctness is checked.
            sigs.insert(cell);
            continue;
        }
        assert!(
            xs.1 - xs.0 + 1 >= icon_px - 2 && ys.1 - ys.0 + 1 >= icon_px - 2,
            "icon {i} is not a full square"
        );
        assert!(
            (xs.1 - xs.0).abs_diff(ys.1 - ys.0) <= 1,
            "icon {i} not square"
        );
        sigs.insert(cell);
    }
    assert_eq!(sigs.len(), total, "icons must differ");
    if let Ok(path) = std::env::var("T125_RASTER") {
        let file = std::fs::File::create(path).unwrap();
        let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w as u32, h as u32);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        let mut wr = enc.write_header().unwrap();
        let data: Vec<u8> = img.iter().flatten().copied().collect();
        wr.write_image_data(&data).unwrap();
    }
}
