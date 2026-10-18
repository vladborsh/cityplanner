//! The cargo trains and ships (T-218). A cargo train (`SimView::cargo_trains`, T-216b) is a
//! freight locomotive and flat wagons with containers or tanks, one entity per car, placed every
//! frame along its leg between two ticks (`TickFraction`) like the intercity train; the cars
//! together are as long as the `cargo_train` vehicle. A cargo ship (`SimView::ships`) is a hull
//! with stacked containers and a bridge at the stern, its bow towards the berth, drawn between
//! its pose now and a tick later on the water off the port (held where the water along its line
//! ends); it fades in as it sails in and out as it sails away (render time only).

use bevy::prelude::*;
use cp_sim::SimView;

use crate::agent_render::TickFraction;
use crate::chunk::MeshData;
use crate::material::VertexColorMaterial;
use crate::problem_render::to_bevy_mesh;
use crate::road::{flat_quad, push_vertex, tri};
use crate::underground::cuboid;

/// Cars of a cargo train: the locomotive and its wagons.
pub const CARGO_CARS: usize = 8;
/// Length of the train and of the ship when the content says nothing, metres.
const DEFAULT_TRAIN: f32 = 140.0;
const DEFAULT_SHIP: (f32, f32) = (120.0, 20.0);
/// Gap between two cars, metres.
const GAP: f32 = 0.8;
const HALF_W: f32 = 1.5;
/// The track surface is this much above the polyline.
const RAIL_LIFT: f32 = 0.6;
/// The ship stands this far off the port's centre (half a 4x4 lot and some water), metres.
const BERTH_OFF: f32 = 20.0;
/// Fraction of the sail in and out over which the ship fades.
const FADE: f32 = 0.3;

const LOCO: [f32; 4] = [0.95, 0.62, 0.10, 1.0];
const LOCO_BAND: [f32; 4] = [0.18, 0.20, 0.24, 1.0];
const FRAME: [f32; 4] = [0.16, 0.16, 0.18, 1.0];
const GLASS: [f32; 4] = [0.12, 0.15, 0.2, 1.0];
const ROOF: [f32; 4] = [0.45, 0.46, 0.50, 1.0];
const TANK: [f32; 4] = [0.22, 0.23, 0.26, 1.0];
const TANK_LIGHT: [f32; 4] = [0.86, 0.86, 0.84, 1.0];
const HULL: [f32; 4] = [0.14, 0.20, 0.34, 1.0];
const HULL_RED: [f32; 4] = [0.62, 0.16, 0.12, 1.0];
const DECK: [f32; 4] = [0.42, 0.40, 0.36, 1.0];
const BRIDGE: [f32; 4] = [0.94, 0.94, 0.92, 1.0];
const FUNNEL: [f32; 4] = [0.85, 0.30, 0.14, 1.0];
/// Container paints, cycled.
const PAINTS: [[f32; 4]; 6] = [
    [0.80, 0.16, 0.14, 1.0],
    [0.18, 0.36, 0.72, 1.0],
    [0.92, 0.92, 0.90, 1.0],
    [0.30, 0.56, 0.36, 1.0],
    [0.86, 0.64, 0.14, 1.0],
    [0.45, 0.45, 0.48, 1.0],
];

/// Length of one car of a cargo train `total` metres long.
pub fn cargo_car_len(total: f32) -> f32 {
    total / CARGO_CARS as f32 - GAP
}

/// The underframe of a wagon `len` long: a dark deck on two bogies, its top at 1.3 m.
fn underframe(m: &mut MeshData, len: f32) {
    cuboid(
        m,
        Vec3::new(0.0, 1.1, 0.0),
        Vec3::new(HALF_W - 0.1, 0.2, len * 0.5),
        FRAME,
    );
    for s in [-1.0f32, 1.0] {
        cuboid(
            m,
            Vec3::new(0.0, 0.45, s * (len * 0.5 - 2.2)),
            Vec3::new(HALF_W - 0.3, 0.45, 1.3),
            FRAME,
        );
    }
}

/// The freight locomotive `len` long on the origin, its nose towards +Z: a narrow orange hood
/// on a dark frame, the full-width cab at the nose with its windows, dark bands at both ends.
pub fn freight_loco_mesh(len: f32) -> MeshData {
    let mut m = MeshData::default();
    underframe(&mut m, len);
    let cab = 3.2;
    let hood = len - cab - 1.0;
    cuboid(
        &mut m,
        Vec3::new(0.0, 2.6, -len * 0.5 + 0.4 + hood * 0.5),
        Vec3::new(HALF_W - 0.45, 1.3, hood * 0.5),
        LOCO,
    );
    let zc = len * 0.5 - 0.6 - cab * 0.5;
    cuboid(
        &mut m,
        Vec3::new(0.0, 2.85, zc),
        Vec3::new(HALF_W, 1.55, cab * 0.5),
        LOCO,
    );
    cuboid(
        &mut m,
        Vec3::new(0.0, 3.35, zc + 0.2),
        Vec3::new(HALF_W + 0.04, 0.5, cab * 0.5 - 0.1),
        GLASS,
    );
    cuboid(
        &mut m,
        Vec3::new(0.0, 4.45, zc),
        Vec3::new(HALF_W - 0.1, 0.08, cab * 0.5 + 0.1),
        ROOF,
    );
    // The nose and the tail: a short hood with a dark band.
    cuboid(
        &mut m,
        Vec3::new(0.0, 2.0, len * 0.5 - 0.3),
        Vec3::new(HALF_W - 0.45, 0.7, 0.3),
        LOCO_BAND,
    );
    cuboid(
        &mut m,
        Vec3::new(0.0, 1.55, 0.0),
        Vec3::new(HALF_W + 0.02, 0.25, len * 0.5 - 0.2),
        LOCO_BAND,
    );
    // Radiator grilles and the exhaust on the hood roof.
    for k in 0..3 {
        cuboid(
            &mut m,
            Vec3::new(0.0, 3.95, -len * 0.5 + 2.0 + k as f32 * 2.2),
            Vec3::new(0.6, 0.06, 0.8),
            ROOF,
        );
    }
    cuboid(
        &mut m,
        Vec3::new(0.0, 4.2, zc - cab * 0.5 - 1.5),
        Vec3::new(0.25, 0.3, 0.25),
        FRAME,
    );
    m
}

/// A flat wagon `len` long carrying two 20 ft containers (one when only one fits) or, with
/// `forty`, one long container, in the paints picked by `k`.
pub fn container_wagon_mesh(len: f32, k: usize, forty: bool) -> MeshData {
    let mut m = MeshData::default();
    underframe(&mut m, len);
    let n = if len >= 13.0 && !forty { 2 } else { 1 };
    let step = (len - 0.6) / n as f32;
    for i in 0..n {
        let z = -len * 0.5 + 0.3 + step * (i as f32 + 0.5);
        cuboid(
            &mut m,
            Vec3::new(0.0, 1.3 + 1.3, z),
            Vec3::new(1.22, 1.3, step * 0.5 - 0.15),
            PAINTS[(k * 2 + i) % PAINTS.len()],
        );
    }
    m
}

/// A tank wagon `len` long: an eight-sided tank lying on the frame with a dome, dark or light.
pub fn tank_wagon_mesh(len: f32, light: bool) -> MeshData {
    let mut m = MeshData::default();
    underframe(&mut m, len);
    let col = if light { TANK_LIGHT } else { TANK };
    let (r, y, h) = (1.35, 2.75, len * 0.5 - 0.6);
    let ring = |k: usize| {
        let a = (k as f32 + 0.5) / 8.0 * std::f32::consts::TAU;
        Vec3::new(r * a.cos(), y + r * a.sin(), 0.0)
    };
    for k in 0..8 {
        let (a, b) = (ring(k), ring(k + 1));
        let n = ((a + b) * 0.5 - Vec3::new(0.0, y, 0.0)).normalize_or_zero();
        flat_quad(
            &mut m,
            [
                a - Vec3::Z * h,
                b - Vec3::Z * h,
                b + Vec3::Z * h,
                a + Vec3::Z * h,
            ],
            n,
            col,
        );
    }
    for s in [-1.0f32, 1.0] {
        let n = Vec3::Z * s;
        let c = push_vertex(&mut m, Vec3::new(0.0, y, s * h), n, col);
        for k in 0..8 {
            let a = push_vertex(&mut m, ring(k) + n * h, n, col);
            let b = push_vertex(&mut m, ring(k + 1) + n * h, n, col);
            tri(&mut m, c, a, b, n);
        }
    }
    cuboid(
        &mut m,
        Vec3::new(0.0, y + r + 0.2, 0.0),
        Vec3::new(0.45, 0.3, 0.45),
        col,
    );
    m
}

/// Which mesh car `i` of a cargo train is: 0 the locomotive, then a pattern of container and
/// tank wagons.
fn car_kind(i: usize) -> usize {
    match i {
        0 => 0,
        _ => [1, 2, 3, 1, 4, 2, 3][(i - 1) % 7],
    }
}

fn car_mesh(kind: usize, len: f32) -> MeshData {
    match kind {
        0 => freight_loco_mesh(len),
        1 => container_wagon_mesh(len, 0, false),
        2 => container_wagon_mesh(len, 1, true),
        3 => tank_wagon_mesh(len, false),
        _ => tank_wagon_mesh(len, true),
    }
}

/// A cargo ship `len` long and `width` wide on the origin, its bow towards +Z and its waterline
/// at y = 0: a dark hull with a red boot top and a pointed bow, the deck, rows of container
/// stacks, the white bridge with wings and the funnel at the stern.
pub fn ship_mesh(len: f32, width: f32) -> MeshData {
    let mut m = MeshData::default();
    let (hw, hl) = (width * 0.5, len * 0.5);
    let bow = 0.16 * len;
    let (keel, deck) = (-1.0, 5.0);
    let body = hl - bow * 0.5;
    // The hull: a box from the stern to where the bow begins, the bow a wedge to a point.
    cuboid(
        &mut m,
        Vec3::new(0.0, (keel + deck) * 0.5, -bow * 0.5),
        Vec3::new(hw, (deck - keel) * 0.5, body),
        HULL,
    );
    cuboid(
        &mut m,
        Vec3::new(0.0, 0.0, -bow * 0.5),
        Vec3::new(hw + 0.05, 0.6, body),
        HULL_RED,
    );
    let tip = |y: f32| Vec3::new(0.0, y, hl);
    let side = |s: f32, y: f32| Vec3::new(s * hw, y, hl - bow);
    for s in [-1.0f32, 1.0] {
        let n = Vec3::new(s * bow, 0.0, hw).normalize_or_zero();
        flat_quad(
            &mut m,
            [side(s, keel), tip(keel), tip(deck + 1.0), side(s, deck)],
            n,
            HULL,
        );
        flat_quad(
            &mut m,
            [side(s, -0.6), tip(-0.6), tip(0.6), side(s, 0.6)],
            n + Vec3::new(0.0, 0.0, 0.01),
            HULL_RED,
        );
    }
    let top = [
        push_vertex(&mut m, side(-1.0, deck), Vec3::Y, DECK),
        push_vertex(&mut m, side(1.0, deck), Vec3::Y, DECK),
        push_vertex(&mut m, tip(deck + 1.0), Vec3::Y, DECK),
    ];
    tri(&mut m, top[0], top[1], top[2], Vec3::Y);
    // The bridge at the stern: a white block, a window band, wings, the funnel behind it.
    let zb = -hl + 0.09 * len;
    cuboid(
        &mut m,
        Vec3::new(0.0, deck + 6.0, zb),
        Vec3::new(hw * 0.75, 6.0, 0.05 * len),
        BRIDGE,
    );
    cuboid(
        &mut m,
        Vec3::new(0.0, deck + 10.8, zb + 0.4),
        Vec3::new(hw * 0.75 + 0.05, 0.7, 0.05 * len),
        GLASS,
    );
    cuboid(
        &mut m,
        Vec3::new(0.0, deck + 11.0, zb + 0.05 * len - 1.0),
        Vec3::new(hw, 0.25, 1.0),
        BRIDGE,
    );
    cuboid(
        &mut m,
        Vec3::new(0.0, deck + 14.5, zb - 0.06 * len),
        Vec3::new(1.4, 2.5, 1.8),
        FUNNEL,
    );
    // The stacks: bays of 20 ft containers from the bridge to the bow, rows across the deck,
    // two to four high.
    let (z0, z1) = (zb + 0.05 * len + 1.5, hl - bow * 0.6);
    let bays = ((z1 - z0) / 6.4).floor() as usize;
    let rows = ((width - 2.0) / 2.5).floor() as usize;
    for b in 0..bays {
        let z = z0 + 6.4 * (b as f32 + 0.5);
        for r in 0..rows {
            let x = (r as f32 - (rows - 1) as f32 * 0.5) * 2.5;
            let high = 2 + (b * 7 + r * 3) % 3;
            let base = (b * 5 + r * 2) % PAINTS.len();
            // Two colours per stack: the lower tiers and the top one.
            let low = (high - 1) as f32 * 2.6;
            cuboid(
                &mut m,
                Vec3::new(x, deck + low * 0.5, z),
                Vec3::new(1.2, low * 0.5, 3.05),
                PAINTS[base],
            );
            cuboid(
                &mut m,
                Vec3::new(x, deck + low + 1.3, z),
                Vec3::new(1.2, 1.3, 3.05),
                PAINTS[(base + 3) % PAINTS.len()],
            );
        }
    }
    m
}

/// How opaque a ship is: it fades in over the first [`FADE`] of its sail in and out over the
/// last of its sail away.
pub fn ship_alpha(phase: cp_transit::ShipPhase, t: f32) -> f32 {
    match phase {
        cp_transit::ShipPhase::Arriving => (t / FADE).clamp(0.0, 1.0),
        cp_transit::ShipPhase::Docked => 1.0,
        cp_transit::ShipPhase::Leaving => ((1.0 - t) / FADE).clamp(0.0, 1.0),
    }
}

/// The water surface under a ship at `c`; where the sail line has left the water (a river
/// bends), the surface off its berth, found along `dir`.
fn water_level(tr: &cp_terrain::Terrain, c: Vec2, berth: Vec2, dir: Vec2) -> f32 {
    let surface = |p: Vec2| {
        let depth = tr.water_depth(p);
        (depth > 0.0).then(|| tr.height(p) + depth)
    };
    surface(c)
        .or_else(|| (0..64).find_map(|i| surface(berth + dir * (4.0 * i as f32))))
        .unwrap_or_else(|| tr.sea_level())
}

/// How far out from `berth` along `dir` the water goes on unbroken, metres (from the first wet
/// point off the quay; at most a sail and a ship length).
fn water_reach(tr: &cp_terrain::Terrain, berth: Vec2, dir: Vec2) -> f32 {
    let step = 4.0;
    let max = ((cp_transit::SAIL_M + 200.0) / step) as usize;
    let wet = |i: usize| tr.is_water(berth + dir * (step * i as f32));
    let Some(first) = (0..max).find(|&i| wet(i)) else {
        return f32::MAX;
    };
    let last = (first..max).take_while(|&i| wet(i)).last().unwrap_or(first);
    step * last as f32
}

#[derive(Component)]
struct CargoCar;

#[derive(Component)]
struct CargoShip;

#[derive(Resource, Default)]
struct CargoRender {
    /// The cars of each train shown, in `SimView::cargo_trains` order.
    trains: Vec<Vec<Entity>>,
    /// The ships shown, in `SimView::ships` order, each with its own material (it fades).
    ships: Vec<(Entity, Handle<VertexColorMaterial>)>,
    /// The train length the car meshes were made for, and the ship size.
    length: f32,
    ship_size: (f32, f32),
    cars: Vec<Handle<Mesh>>,
    ship: Option<Handle<Mesh>>,
    material: Option<Handle<VertexColorMaterial>>,
}

/// What `sync_cargo` moves on a car or a ship.
type Placed = (&'static mut Transform, &'static mut Visibility);

#[allow(clippy::too_many_arguments)]
fn sync_cargo(
    view: Option<Res<SimView>>,
    fraction: Res<TickFraction>,
    mut st: ResMut<CargoRender>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<VertexColorMaterial>>,
    mut cars: Query<Placed, (With<CargoCar>, Without<CargoShip>)>,
    mut ships: Query<Placed, (With<CargoShip>, Without<CargoCar>)>,
) {
    let Some(view) = view.as_deref() else {
        return;
    };
    let frac = fraction.0.clamp(0.0, 1.0);
    let content = view.content.as_deref();
    let total = content
        .and_then(|c| c.vehicle_id("cargo_train").map(|id| c.vehicle(id).length))
        .unwrap_or(DEFAULT_TRAIN);
    let len = cargo_car_len(total);
    if st.length != total {
        for e in st.trains.drain(..).flatten() {
            commands.entity(e).despawn();
        }
        st.length = total;
        st.cars = (0..5)
            .map(|k| meshes.add(to_bevy_mesh(car_mesh(k, len))))
            .collect();
    }
    let material = st
        .material
        .get_or_insert_with(|| materials.add(VertexColorMaterial::opaque(0.6, 0.1)))
        .clone();

    // Trains: one set of car entities per train, the extra sets despawned.
    while st.trains.len() > view.cargo_trains.len() {
        for e in st.trains.pop().unwrap_or_default() {
            commands.entity(e).despawn();
        }
    }
    for (n, train) in view.cargo_trains.iter().enumerate() {
        let pose = &train.pose;
        let head = pose.s + (pose.next_s - pose.s) * frac;
        if st.trains.len() == n {
            st.trains.push(Vec::new());
        }
        for i in 0..CARGO_CARS {
            let centre = head - len * 0.5 - i as f32 * (len + GAP);
            let (p, dir) = pose.at(centre);
            let flat = Vec3::new(dir.x, 0.0, dir.z);
            let rot = if flat.length_squared() > 1e-6 {
                Quat::from_rotation_arc(Vec3::Z, flat.normalize())
            } else {
                Quat::IDENTITY
            };
            let tf = Transform::from_translation(p + Vec3::Y * RAIL_LIFT).with_rotation(rot);
            let visible = if centre + len * 0.5 > 0.0 {
                Visibility::Inherited
            } else {
                Visibility::Hidden
            };
            if let Some(&e) = st.trains[n].get(i)
                && let Ok((mut t, mut v)) = cars.get_mut(e)
            {
                *t = tf;
                *v = visible;
            } else if st.trains[n].len() == i {
                let kind = car_kind(i);
                let e = commands
                    .spawn((
                        Mesh3d(st.cars[kind].clone()),
                        MeshMaterial3d(material.clone()),
                        tf,
                        visible,
                        CargoCar,
                        bevy::light::NotShadowCaster,
                        bevy::camera::visibility::NoFrustumCulling,
                        Name::new(if i == 0 {
                            "cargo locomotive"
                        } else {
                            "cargo wagon"
                        }),
                    ))
                    .id();
                st.trains[n].push(e);
            }
        }
    }

    // Ships.
    let size = content
        .and_then(|c| {
            c.vehicle_id("cargo_ship").map(|id| {
                let v = c.vehicle(id);
                (v.length, v.width)
            })
        })
        .unwrap_or(DEFAULT_SHIP);
    if st.ship_size != size {
        for (e, _) in st.ships.drain(..) {
            commands.entity(e).despawn();
        }
        st.ship_size = size;
        st.ship = Some(meshes.add(to_bevy_mesh(ship_mesh(size.0, size.1))));
    }
    while st.ships.len() > view.ships.len() {
        if let Some((e, _)) = st.ships.pop() {
            commands.entity(e).despawn();
        }
    }
    let terrain = view.terrain.as_deref();
    for (n, ship) in view.ships.iter().enumerate() {
        let p = ship.pos().lerp(ship.next_pos(), frac);
        let t = ship.t + (ship.next_t - ship.t) * frac;
        let alpha = ship_alpha(ship.phase, t);
        // Bow towards the berth, standing off it by half a lot and half the ship, and never
        // further out than the water reaches along its line (a river bends away).
        let out = (p - ship.berth).length() + BERTH_OFF + size.0 * 0.5;
        let reach = terrain.map_or(f32::MAX, |tr| water_reach(tr, ship.berth, ship.dir));
        let c = ship.berth + ship.dir * out.min(reach - size.0 * 0.5).max(BERTH_OFF + size.0 * 0.5);
        let water = terrain.map_or(0.0, |tr| water_level(tr, c, ship.berth, ship.dir));
        let rot = Quat::from_rotation_arc(
            Vec3::Z,
            Vec3::new(-ship.dir.x, 0.0, -ship.dir.y).normalize_or(Vec3::Z),
        );
        let tf = Transform::from_translation(Vec3::new(c.x, water, c.y)).with_rotation(rot);
        let visible = if alpha > 0.01 {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        if let Some((e, mat)) = st.ships.get(n)
            && let Ok((mut tr, mut v)) = ships.get_mut(*e)
        {
            *tr = tf;
            *v = visible;
            // Touch the material only while the ship fades (a change re-uploads it).
            let want = if alpha < 0.999 { alpha } else { 1.0 };
            if materials.get(mat).is_some_and(|mm| mm.fade.x != want)
                && let Some(mut mm) = materials.get_mut(mat)
            {
                mm.set_see_through(alpha < 0.999, alpha);
            }
        } else if st.ships.len() == n {
            let mut mm = VertexColorMaterial::opaque(0.6, 0.1);
            mm.set_see_through(alpha < 0.999, alpha);
            let mat = materials.add(mm);
            let e = commands
                .spawn((
                    Mesh3d(st.ship.clone().expect("ship mesh made")),
                    MeshMaterial3d(mat.clone()),
                    tf,
                    visible,
                    CargoShip,
                    bevy::camera::visibility::NoFrustumCulling,
                    Name::new("cargo ship"),
                ))
                .id();
            st.ships.push((e, mat));
        }
    }
}

pub struct CargoRenderPlugin;

impl Plugin for CargoRenderPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CargoRender>()
            .init_resource::<TickFraction>()
            .add_systems(Update, sync_cargo);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn extent(m: &MeshData) -> (Vec3, Vec3) {
        crate::road::bounds(m)
    }

    fn sound(m: &MeshData) -> bool {
        m.positions.iter().all(|p| p.iter().all(|v| v.is_finite()))
            && m.indices.iter().all(|&i| (i as usize) < m.positions.len())
    }

    /// T-218: the cargo train is as long as the `cargo_train` vehicle; its locomotive and its
    /// wagons are each as long as a car, sound, and pictures of their own (in shape or colour),
    /// the locomotive not the intercity's.
    #[test]
    fn the_cargo_train_is_as_long_as_its_vehicle_and_its_cars_differ() {
        let db = cp_content::ContentDb::load_dir(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets/content"
        ))
        .unwrap();
        let def = db.vehicle(db.vehicle_id("cargo_train").unwrap());
        let len = cargo_car_len(def.length);
        let total = CARGO_CARS as f32 * len + (CARGO_CARS as f32 - 1.0) * GAP;
        assert!(
            (total - def.length).abs() <= GAP,
            "{total} m for {}",
            def.length
        );
        let meshes: Vec<MeshData> = (0..5).map(|k| car_mesh(k, len)).collect();
        for (k, m) in meshes.iter().enumerate() {
            assert!(sound(m), "{k}");
            let (lo, hi) = extent(m);
            assert!(
                hi.z - lo.z <= len + 0.05 && hi.z - lo.z > len - 0.5,
                "{k}: {lo} {hi}"
            );
            assert!(hi.x - lo.x <= 2.0 * HALF_W + 0.2, "{k}");
            assert!(lo.y >= -1e-3 && hi.y < 5.0, "{k}");
        }
        for a in 0..meshes.len() {
            for b in a + 1..meshes.len() {
                assert!(
                    meshes[a].positions != meshes[b].positions
                        || meshes[a].colors != meshes[b].colors,
                    "{a} {b}"
                );
            }
        }
        let coach = crate::train_render::loco_mesh(crate::train_render::car_len(def.length));
        assert_ne!(
            meshes[0].colors, coach.colors,
            "not the intercity locomotive"
        );
        // Every car kind turns up in a train.
        let kinds: std::collections::BTreeSet<usize> = (0..CARGO_CARS).map(car_kind).collect();
        assert_eq!(kinds.len(), 5);
    }

    /// T-218: the ship is as long and wide as the `cargo_ship` vehicle, sits on its waterline,
    /// carries containers above the deck, and fades in and out only while it sails.
    #[test]
    fn the_cargo_ship_fits_its_vehicle_and_fades_while_it_sails() {
        let db = cp_content::ContentDb::load_dir(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets/content"
        ))
        .unwrap();
        let def = db.vehicle(db.vehicle_id("cargo_ship").unwrap());
        let m = ship_mesh(def.length, def.width);
        assert!(sound(&m));
        let (lo, hi) = extent(&m);
        assert!((hi.z - lo.z - def.length).abs() < 0.1, "{lo} {hi}");
        assert!((hi.x - lo.x - def.width).abs() < 0.2, "{lo} {hi}");
        assert!(lo.y < 0.0 && hi.y > 12.0, "{lo} {hi}");
        let colours: std::collections::BTreeSet<_> = m
            .colors
            .iter()
            .map(|c| c.map(|v| (v * 100.0) as i32))
            .collect();
        assert!(
            PAINTS
                .iter()
                .all(|p| colours.contains(&p.map(|v| (v * 100.0) as i32)))
        );
        use cp_transit::ShipPhase::*;
        assert_eq!(ship_alpha(Arriving, 0.0), 0.0);
        assert_eq!(ship_alpha(Arriving, 0.5), 1.0);
        assert_eq!(ship_alpha(Docked, 0.3), 1.0);
        assert_eq!(ship_alpha(Leaving, 0.5), 1.0);
        assert_eq!(ship_alpha(Leaving, 1.0), 0.0);
        assert!(ship_alpha(Arriving, 0.15) > 0.0 && ship_alpha(Arriving, 0.15) < 1.0);
    }
}
