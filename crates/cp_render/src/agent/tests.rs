use bevy_math::{Vec2, Vec3, Vec4};
use cp_sim::{PedestrianPose, VehiclePose};

use super::*;

fn everything() -> [Vec4; 6] {
    [Vec4::new(0.0, 0.0, 0.0, 1e9); 6]
}

fn cull(y: f32, alpha: f32) -> Cull {
    Cull {
        height: y,
        planes: everything(),
        alpha,
        time: 0.0,
    }
}

fn veh(key: u64, x: f32, yaw: f32) -> VehiclePose {
    VehiclePose {
        key,
        kind: 0,
        pos: Vec3::new(x, 0.0, 0.0),
        yaw,
        speed: 10.0,
        tint: None,
    }
}

fn ped(key: u64, x: f32) -> PedestrianPose {
    PedestrianPose {
        key,
        pos: Vec2::new(x, 5.0),
        portal: None,
        room: 0.0,
    }
}

#[test]
fn vehicles_interpolate_between_snapshots() {
    let prev = [veh(1, 0.0, 0.0), veh(2, 10.0, 0.0)];
    let cur = [veh(1, 10.0, 0.0), veh(2, 20.0, 0.0), veh(3, 7.0, 0.0)];
    let mut out = AgentInstances::default();
    build_vehicles(
        &prev,
        &cur,
        &VehicleLooks::default(),
        &cull(100.0, 0.25),
        &mut out,
    );
    let xs: Vec<f32> = out.vehicles[0][0].iter().map(|i| i.pos_yaw[0]).collect();
    // 1 and 2 lerp a quarter of the way; 3 is new and shows at its current pose.
    assert_eq!(xs, vec![2.5, 12.5, 7.0]);
}

#[test]
fn heading_takes_the_short_way_round() {
    let tau = std::f32::consts::TAU;
    let prev = [veh(1, 0.0, tau - 0.1)];
    let cur = [veh(1, 0.0, 0.1)];
    let mut out = AgentInstances::default();
    build_vehicles(
        &prev,
        &cur,
        &VehicleLooks::default(),
        &cull(100.0, 0.5),
        &mut out,
    );
    let yaw = out.vehicles[0][0][0].pos_yaw[3];
    assert!((yaw - tau).abs() < 1e-4 || yaw.abs() < 1e-4, "{yaw}");
}

#[test]
fn teleports_are_not_interpolated() {
    let prev = [veh(1, 0.0, 0.0)];
    let cur = [veh(1, 500.0, 0.0)];
    let mut out = AgentInstances::default();
    build_vehicles(
        &prev,
        &cur,
        &VehicleLooks::default(),
        &cull(100.0, 0.5),
        &mut out,
    );
    assert_eq!(out.vehicles[0][0][0].pos_yaw[0], 500.0);
}

#[test]
fn camera_height_gates_each_kind() {
    let v = [veh(1, 0.0, 0.0)];
    let p = [ped(1, 0.0)];
    let looks = VehicleLooks::default();
    let count = |y: f32| {
        let mut out = AgentInstances::default();
        build_vehicles(&v, &v, &looks, &cull(y, 0.0), &mut out);
        build_pedestrians(
            &p,
            &p,
            |_, _| 0.0,
            |_, _, lo, hi| (lo, hi),
            |_| None,
            &cull(y, 0.0),
            &mut Headings::default(),
            &mut out,
        );
        (out.vehicle_count(), out.pedestrian_count())
    };
    assert_eq!(count(100.0), (1, 1));
    assert_eq!(count(PEDESTRIAN_MAX_HEIGHT + 1.0), (1, 0));
    assert_eq!(count(VEHICLE_MAX_HEIGHT + 1.0), (0, 0));
}

#[test]
fn detail_level_follows_camera_height() {
    let v = [veh(1, 0.0, 0.0)];
    let mut out = AgentInstances::default();
    build_vehicles(&v, &v, &VehicleLooks::default(), &cull(50.0, 0.0), &mut out);
    assert_eq!((out.vehicles[0][0].len(), out.vehicles[0][1].len()), (1, 0));
    out.clear();
    build_vehicles(
        &v,
        &v,
        &VehicleLooks::default(),
        &cull(400.0, 0.0),
        &mut out,
    );
    assert_eq!((out.vehicles[0][0].len(), out.vehicles[0][1].len()), (0, 1));
}

#[test]
fn frustum_culls_agents_outside_it() {
    // Single plane x >= 0 (inside), others open.
    let mut planes = everything();
    planes[0] = Vec4::new(1.0, 0.0, 0.0, 0.0);
    let c = Cull {
        planes,
        ..cull(100.0, 0.0)
    };
    let v = [veh(1, 50.0, 0.0), veh(2, -50.0, 0.0)];
    let mut out = AgentInstances::default();
    build_vehicles(&v, &v, &VehicleLooks::default(), &c, &mut out);
    assert_eq!(out.vehicle_count(), 1);
}

#[test]
fn pedestrians_face_their_motion_and_stand_still_without_sway() {
    let prev = [ped(1, 0.0), ped(2, 3.0)];
    let cur = [ped(1, 1.0), ped(2, 3.0)];
    let mut out = AgentInstances::default();
    build_pedestrians(
        &prev,
        &cur,
        |_, _| 2.0,
        |_, _, lo, hi| (lo, hi),
        |_| None,
        &cull(50.0, 1.0),
        &mut Headings::default(),
        &mut out,
    );
    let i = &out.pedestrians[0];
    assert_eq!(i.len(), 2);
    assert!(
        (i[0].pos_yaw[3] - std::f32::consts::FRAC_PI_2).abs() < 1e-4,
        "+x motion"
    );
    assert_eq!(i[0].pos_yaw[1], 2.0, "stands on the ground");
    assert!(i[0].color_sway[3] > 0.0 && i[1].color_sway[3] == 0.0);
}

#[test]
fn vehicle_sizes_come_from_content() {
    let db = cp_content::ContentDb::load_dir(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../assets/content"
    ))
    .expect("content");
    let looks = VehicleLooks::from_content(&db);
    let car = db.vehicle(db.vehicle_id("car_basic").expect("car_basic"));
    assert_eq!(looks.sizes[0].z, car.length);
    assert_eq!(looks.sizes[0].x, car.width);
    // The intercity train (kind 6) has its own size, not the goods truck's it once fell back to.
    let train = db.vehicle(db.vehicle_id("intercity").expect("intercity"));
    assert_eq!(train.kind as usize, 6);
    assert_eq!(looks.sizes[6].z, train.length);
    assert_ne!(looks.sizes[6], looks.sizes[5]);
    assert_ne!(
        mesh::vehicle_mesh(6, 0).positions.len(),
        mesh::vehicle_mesh(5, 0).positions.len()
    );
}

#[test]
fn every_vehicle_kind_has_a_look() {
    assert_eq!(VEHICLE_KINDS, cp_content::VehicleKind::COUNT);
    for kind in cp_content::VehicleKind::ALL {
        assert!((kind as usize) < VEHICLE_KINDS);
    }
}

#[test]
fn meshes_are_well_formed() {
    for (k, d) in (0..VEHICLE_KINDS).flat_map(|k| [(k, 0), (k, 1)]) {
        let m = mesh::vehicle_mesh(k, d);
        assert_eq!(m.positions.len(), m.normals.len());
        assert_eq!(m.positions.len(), m.colors.len());
        assert!(m.indices.iter().all(|&i| (i as usize) < m.positions.len()));
        assert!(m.triangle_count() >= 12);
    }
    assert!(mesh::vehicle_mesh(0, 0).triangle_count() > mesh::vehicle_mesh(0, 1).triangle_count());
    assert!(mesh::pedestrian_mesh(0).triangle_count() > mesh::pedestrian_mesh(1).triangle_count());
}

/// The §14 budget scene, all of it in view: 16k cars and 20k pedestrians. Prints the CPU
/// time of the instance build (the budget is 3 ms with the rest of the render prepare);
/// asserts only the counts, since timing varies by machine.
#[test]
fn budget_scene_builds_every_instance() {
    let cars: Vec<VehiclePose> = (0..16_000u64)
        .map(|k| veh(k, (k % 400) as f32, 0.3))
        .collect();
    let cars_prev: Vec<VehiclePose> = cars
        .iter()
        .map(|c| VehiclePose {
            pos: c.pos - Vec3::X,
            ..*c
        })
        .collect();
    let peds: Vec<PedestrianPose> = (0..20_000u64).map(|k| ped(k, (k % 500) as f32)).collect();
    let peds_prev: Vec<PedestrianPose> = peds
        .iter()
        .map(|p| PedestrianPose {
            pos: p.pos - Vec2::X * 0.5,
            ..*p
        })
        .collect();
    let looks = VehicleLooks::default();
    let c = cull(100.0, 0.5);
    let mut out = AgentInstances::default();
    let mut best = std::time::Duration::MAX;
    for _ in 0..5 {
        out.clear();
        let t = std::time::Instant::now();
        build_vehicles(&cars_prev, &cars, &looks, &c, &mut out);
        build_pedestrians(
            &peds_prev,
            &peds,
            |_, _| 0.0,
            |_, _, lo, hi| (lo, hi),
            |_| None,
            &c,
            &mut Headings::default(),
            &mut out,
        );
        best = best.min(t.elapsed());
    }
    println!("instance build, 16k cars + 20k pedestrians: {best:?}");
    assert_eq!(
        (out.vehicle_count(), out.pedestrian_count()),
        (16_000, 20_000)
    );
}

#[test]
fn a_pedestrian_that_stops_keeps_facing_the_way_it_walked() {
    let mut headings = Headings::default();
    let mut yaw = |prev: &[PedestrianPose], cur: &[PedestrianPose]| {
        let mut out = AgentInstances::default();
        build_pedestrians(
            prev,
            cur,
            |_, _| 0.0,
            |_, _, lo, hi| (lo, hi),
            |_| None,
            &cull(50.0, 0.5),
            &mut headings,
            &mut out,
        );
        out.pedestrians[0]
            .iter()
            .map(|i| i.pos_yaw[3])
            .collect::<Vec<_>>()
    };
    // Walks +x, then stands (waiting at a crossing); a second one is seen standing first.
    let walking = yaw(&[ped(1, 0.0)], &[ped(1, 1.0)]);
    let standing = yaw(&[ped(1, 1.0)], &[ped(1, 1.0), ped(2, 5.0)]);
    let still = yaw(&[ped(1, 1.0), ped(2, 5.0)], &[ped(1, 1.0), ped(2, 5.0)]);
    assert!((walking[0] - std::f32::consts::FRAC_PI_2).abs() < 1e-4);
    assert_eq!(standing[0], walking[0], "keeps its heading when it stops");
    assert_eq!(still, standing, "the newcomer's random heading is kept too");
}

#[test]
fn the_walk_phase_follows_the_distance_walked_not_the_clock() {
    let phase = |from: f32, to: f32, time: f32, alpha: f32| {
        let mut out = AgentInstances::default();
        let c = Cull {
            time,
            ..cull(50.0, alpha)
        };
        build_pedestrians(
            &[ped(1, from)],
            &[ped(1, to)],
            |_, _| 0.0,
            |_, _, lo, hi| (lo, hi),
            |_| None,
            &c,
            &mut Headings::default(),
            &mut out,
        );
        out.pedestrians[0][0].scale_phase[3]
    };
    // Half a 2 m step further along the same walk is 1 / STRIDE of a cycle more, whatever the time.
    let d = phase(0.0, 2.0, 0.0, 0.75) - phase(0.0, 2.0, 7.0, 0.25);
    let want = std::f32::consts::TAU / STRIDE;
    assert!((d - want).abs() < 1e-3, "{d} vs {want}");
}

#[test]
fn the_walking_mesh_has_two_legs_and_two_arms_and_the_far_one_stays_one_box() {
    let m = mesh::pedestrian_mesh(0);
    let alphas = |lo: f32, hi: f32| m.colors.iter().filter(|c| c[3] > lo && c[3] < hi).count() / 24;
    assert_eq!(alphas(1.5, 2.5), 2, "legs");
    assert_eq!(alphas(4.5, 5.5), 2, "arms");
    assert!(mesh::pedestrian_mesh(1).colors.iter().all(|c| c[3] <= 1.0));
}

fn walker(key: u64, x: f32, room: f32, portal: Option<cp_sim::Portal>) -> PedestrianPose {
    PedestrianPose {
        key,
        pos: Vec2::new(x, 0.0),
        portal,
        room,
    }
}

fn no_bays(_: Vec2, _: Vec2, lo: f32, hi: f32) -> (f32, f32) {
    (lo, hi)
}

fn no_door(_: cp_building::BuildingId) -> Option<Vec2> {
    None
}

/// §25.5 item 4: 200 walkers along +x on a 4,5 m sidewalk (room 1,65 m) whose bay leaves the
/// side below −1 m clear of nobody: they spread over the whole clear width, six equal bins each
/// hold 5 % or more, nobody stands in the bay, and one key keeps its place from frame to frame.
#[test]
fn walkers_spread_over_the_sidewalk_and_stay_out_of_the_bay() {
    let side = Vec2::Y;
    let room = (4.5 - 1.2) * 0.5;
    let (edge, top) = (-1.0, room + 0.35);
    let bay = |_: Vec2, _: Vec2, lo: f32, hi: f32| (lo.max(edge), hi);
    let mut bins = [0u32; 6];
    for key in 0..200u64 {
        let x = key as f32 * 3.7;
        let c = walker(key << 32, x, room, None);
        let y = pedestrian_drawn(&c, side, x, &bay, &no_door).y;
        assert!(
            (edge - 1e-4..=top + 1e-4).contains(&y),
            "walker {key} at {y}"
        );
        bins[(((y - edge) / (top - edge) * 6.0) as usize).min(5)] += 1;
        // Half a metre later: the drift moves it a little, never across the sidewalk.
        let c2 = walker(key << 32, x + 0.5, room, None);
        let y2 = pedestrian_drawn(&c2, side, x + 0.5, &bay, &no_door).y;
        assert!((y2 - y).abs() < 0.2, "walker {key} jumped {y} -> {y2}");
        assert_eq!(y, pedestrian_drawn(&c, side, x, &bay, &no_door).y);
    }
    assert!(bins.iter().all(|&b| b >= 10), "{bins:?}");
}

/// §25.5 item 5: leaving, the walker is drawn in the doorway at progress 0, exactly where it is
/// drawn without a door at progress 1, and gets steadily farther from the door in between;
/// entering mirrors it.
#[test]
fn a_walker_blends_between_its_door_and_the_sidewalk() {
    let id = cp_core::Id::new(4, 0);
    let door_at = Vec2::new(10.0, 9.0);
    let door = |b: cp_building::BuildingId| (b == id).then_some(door_at);
    let at = |progress: f32, leaving: bool| {
        let portal = cp_sim::Portal {
            building: id,
            progress,
            leaving,
        };
        pedestrian_drawn(
            &walker(7, 12.0, 1.0, Some(portal)),
            Vec2::Y,
            12.0,
            &no_bays,
            &door,
        )
    };
    let free = pedestrian_drawn(
        &walker(7, 12.0, 1.0, None),
        Vec2::Y,
        12.0,
        &no_bays,
        &no_door,
    );
    assert!(at(0.0, true).distance(door_at) < 0.1);
    assert!(at(1.0, true).distance(free) < 1e-5);
    assert!(at(0.0, false).distance(free) < 1e-5);
    assert!(at(1.0, false).distance(door_at) < 0.1);
    let mut last = 0.0;
    for i in 1..=10 {
        let d = at(i as f32 / 10.0, true).distance(door_at);
        assert!(d > last, "progress {i}/10: {d} after {last}");
        last = d;
    }
    // Without a known door the portal changes nothing.
    let lost = cp_sim::Portal {
        building: cp_core::Id::new(5, 0),
        progress: 0.0,
        leaving: true,
    };
    let p = pedestrian_drawn(
        &walker(7, 12.0, 1.0, Some(lost)),
        Vec2::Y,
        12.0,
        &no_bays,
        &door,
    );
    assert_eq!(p, free);
}

/// A walker stepping out of its door faces the way it is drawn moving (towards the sidewalk),
/// not along the sidewalk the sim moves it on.
#[test]
fn a_walker_leaving_its_door_faces_the_way_it_is_drawn() {
    let id = cp_core::Id::new(4, 0);
    let door = |_: cp_building::BuildingId| Some(Vec2::new(0.0, 10.0));
    let pose = |x: f32, progress: f32| {
        walker(
            9,
            x,
            0.0,
            Some(cp_sim::Portal {
                building: id,
                progress,
                leaving: true,
            }),
        )
    };
    let mut out = AgentInstances::default();
    build_pedestrians(
        &[pose(0.0, 0.0)],
        &[pose(2.8, 0.28)],
        |_, _| 0.0,
        no_bays,
        door,
        &cull(50.0, 0.5),
        &mut Headings::default(),
        &mut out,
    );
    let i = out.pedestrians[0][0];
    // Drawn from (0, 10) towards the sidewalk at y = 0: mostly −z.
    let yaw = i.pos_yaw[3];
    assert!(yaw.cos() < -0.5, "yaw {yaw}");
    assert!(
        i.pos_yaw[2] > 5.0,
        "still in the front yard: {:?}",
        i.pos_yaw
    );
}
