//! The Underground view (T-164): the ground, the roads and the buildings turn see-through; the
//! tunnels show as pipes of their own, the metro lines as ribbons along the track and the metro
//! trains run in them. Outside the view none of this is drawn.
//!
//! The geometry is a pure function of the net graph and the content ([`tunnel_mesh`]), of the
//! routes ([`route_mesh`]) and of a colour ([`train_mesh`]). The Bevy side keeps the entities
//! while the view is on and removes them when it is off.
//!
//! Trains are abstract timed runs in the sim (T-163): their pose comes from
//! `SimView::metro_trains` (an interpolation along the leg of the plan on the track lanes) and
//! is drawn between two ticks by [`TickFraction`].

use std::collections::BTreeSet;
use std::sync::Arc;

use bevy::prelude::*;
use cp_content::{ContentDb, RoadClass};
use cp_net::{Elevation, NetGraph, SegmentId};
use cp_sim::SimView;
use cp_transit::{LINE_PALETTE_RGB, TransitView};

use crate::agent_render::TickFraction;
use crate::building_render::BuildingChunk;
use crate::chunk::MeshData;
use crate::material::VertexColorMaterial;
use crate::overlay::{LineGhost, line_color, to_bevy_mesh};
use crate::road_render::RoadChunk;
use crate::terrain::{SeaPlane, TerrainChunk, WaterSurface};
use crate::utility_render::{spawn_mesh, tube_faces};

/// Which of the two ways the Underground view is on. The View menu sets `manual`; the metro
/// tools (track, station, line) set `auto` while one of them is active.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UndergroundView {
    pub manual: bool,
    pub auto: bool,
}

impl UndergroundView {
    pub fn on(&self) -> bool {
        self.manual || self.auto
    }
}

/// Alpha of the ground, the roads and the buildings in the view.
pub const GROUND_ALPHA: f32 = 0.15;
pub const ROAD_ALPHA: f32 = 0.25;
pub const BUILDING_ALPHA: f32 = 0.3;
pub const WATER_ALPHA: f32 = 0.06;

/// Added to the sort distance of the tunnel walls and the ribbons so that they are drawn after
/// the see-through ground, the roads and the sea (blended layers are sorted back to front).
const UNDER_SORT_BIAS: f32 = 1.0e6;

/// Faces around a tunnel.
const TUNNEL_FACES: usize = 14;
/// Ring spacing along a tunnel, metres.
const TUNNEL_STEP: f32 = 6.0;
/// Alpha of a tunnel wall: the trains show through it.
const TUNNEL_ALPHA: f32 = 0.5;
const TUNNEL_COLOR: [f32; 3] = [0.55, 0.68, 0.9];
const PLATFORM_COLOR: [f32; 3] = [0.95, 0.85, 0.5];
/// Half width of a line's ribbon in the tunnel, metres.
const RIBBON_HALF: f32 = 1.3;

/// The walls of every tunnel segment as a tube of the road's width; the segments in
/// `platforms` are lighter and a little wider.
pub fn tunnel_mesh(
    net: &NetGraph,
    content: &ContentDb,
    platforms: &BTreeSet<SegmentId>,
) -> MeshData {
    let mut m = MeshData::default();
    for (id, seg) in net
        .segments()
        .filter(|(_, s)| s.elevation == Elevation::Tunnel)
    {
        let def = content.road(seg.road);
        let platform = platforms.contains(&id);
        let radius = def.width * 0.5 + if platform { 1.0 } else { 0.5 };
        let [r, g, b] = if platform {
            PLATFORM_COLOR
        } else if def.class == RoadClass::Rail {
            TUNNEL_COLOR
        } else {
            [0.55, 0.5, 0.45]
        };
        let table = seg.curve.arc_table();
        let len = table.length();
        let n = ((len / TUNNEL_STEP).ceil() as usize).max(1);
        let rings: Vec<(Vec3, Vec3)> = (0..=n)
            .map(|i| {
                let t = table.s_to_t(len * i as f32 / n as f32);
                (seg.curve.point(t), seg.curve.tangent(t))
            })
            .collect();
        tube_faces(
            &mut m,
            &rings,
            radius,
            [r, g, b, TUNNEL_ALPHA],
            TUNNEL_FACES,
        );
    }
    m
}

/// A ribbon of `half` either side along `points` at `heights` (+ `lift`), facing up.
fn ribbon3(
    m: &mut MeshData,
    points: &[Vec2],
    heights: &[f32],
    half: f32,
    lift: f32,
    color: [f32; 4],
) {
    let mut prev: Option<[[f32; 3]; 2]> = None;
    let mut last: Option<Vec2> = None;
    for (i, &p) in points.iter().enumerate() {
        if last.is_some_and(|l| l.distance_squared(p) < 0.01) {
            continue;
        }
        let a = points[i.saturating_sub(1)];
        let b = points[(i + 1).min(points.len() - 1)];
        let dir = (b - a).normalize_or_zero();
        let side = Vec2::new(dir.y, -dir.x) * half;
        let y = heights.get(i).copied().unwrap_or(0.0) + lift;
        let ring = [
            [p.x + side.x, y, p.y + side.y],
            [p.x - side.x, y, p.y - side.y],
        ];
        if let Some(pr) = prev {
            let base = m.positions.len() as u32;
            for q in [pr[0], ring[0], ring[1], pr[1]] {
                m.positions.push(q);
                m.normals.push([0.0, 1.0, 0.0]);
                m.colors.push(color);
            }
            let up = {
                let v = |k: u32| Vec3::from(m.positions[(base + k) as usize]);
                (v(1) - v(0)).cross(v(2) - v(0)).y >= 0.0
            };
            if up {
                m.indices
                    .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
            } else {
                m.indices
                    .extend_from_slice(&[base, base + 2, base + 1, base, base + 3, base + 2]);
            }
        }
        prev = Some(ring);
        last = Some(p);
    }
}

/// Height of a line's ribbon above the track axis: on top of the tunnel wall.
const RIBBON_LIFT: f32 = 5.1;

/// The ribbons of the metro lines of `t`, each in its colour on top of its tunnel.
pub fn route_mesh(t: &TransitView) -> MeshData {
    let mut m = MeshData::default();
    for (n, (id, route)) in t.routes.iter().enumerate() {
        let Some(line) = t.net.lines.get(*id).filter(|_| route.underground()) else {
            continue;
        };
        let alpha = if route.ok { 0.95 } else { 0.5 };
        ribbon3(
            &mut m,
            &route.points,
            &route.heights,
            RIBBON_HALF,
            RIBBON_LIFT + 0.03 * (n % 8) as f32,
            line_color(line.color, alpha),
        );
    }
    m
}

/// The ribbon of the line tool's ghost route, when it is a route on the track.
pub fn ghost_route_mesh(g: &LineGhost) -> MeshData {
    let mut m = MeshData::default();
    if !g.heights.is_empty() {
        ribbon3(
            &mut m,
            &g.route,
            &g.heights,
            RIBBON_HALF,
            RIBBON_LIFT + 0.3,
            line_color(g.color, 0.8),
        );
    }
    m
}

/// An axis-aligned box of half sizes `h` centred on `c`.
pub(crate) fn cuboid(m: &mut MeshData, c: Vec3, h: Vec3, color: [f32; 4]) {
    let faces: [(Vec3, Vec3, Vec3); 6] = [
        (Vec3::X, Vec3::Y, Vec3::Z),
        (Vec3::NEG_X, Vec3::Y, Vec3::NEG_Z),
        (Vec3::Y, Vec3::Z, Vec3::X),
        (Vec3::NEG_Y, Vec3::X, Vec3::Z),
        (Vec3::Z, Vec3::Y, Vec3::NEG_X),
        (Vec3::NEG_Z, Vec3::Y, Vec3::X),
    ];
    for (n, u, v) in faces {
        let base = m.positions.len() as u32;
        let (cu, cv, cn) = (u * h.dot(u.abs()), v * h.dot(v.abs()), n * h.dot(n.abs()));
        for (a, b) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
            m.positions.push((c + cn + cu * a + cv * b).to_array());
            m.normals.push(n.to_array());
            m.colors.push(color);
        }
        // `u × v` may point either way round `n`: wind to face outwards.
        if u.cross(v).dot(n) > 0.0 {
            m.indices
                .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        } else {
            m.indices
                .extend_from_slice(&[base, base + 2, base + 1, base, base + 3, base + 2]);
        }
    }
}

/// Cars of a train and their length, gap, width and height, metres.
pub const CARS: usize = 3;
pub const CAR_LEN: f32 = 18.0;
pub const CAR_GAP: f32 = 1.2;
const CAR_HALF_W: f32 = 1.5;
const CAR_HALF_H: f32 = 1.6;

/// A metro train of `CARS` cars in the colour of line `color`, on the origin, running along
/// +Z, its floor on the track axis.
pub fn train_mesh(color: u8) -> MeshData {
    let [r, g, b] = LINE_PALETTE_RGB[usize::from(color) % LINE_PALETTE_RGB.len()];
    let body = [
        f32::from(r) / 255.0,
        f32::from(g) / 255.0,
        f32::from(b) / 255.0,
        1.0,
    ];
    let glass = [0.12, 0.15, 0.2, 1.0];
    let mut m = MeshData::default();
    let total = CARS as f32 * CAR_LEN + (CARS as f32 - 1.0) * CAR_GAP;
    for i in 0..CARS {
        let z = -total * 0.5 + CAR_LEN * 0.5 + i as f32 * (CAR_LEN + CAR_GAP);
        let c = Vec3::new(0.0, CAR_HALF_H + 0.3, z);
        cuboid(
            &mut m,
            c,
            Vec3::new(CAR_HALF_W, CAR_HALF_H, CAR_LEN * 0.5),
            body,
        );
        // A dark window band a little proud of the sides.
        cuboid(
            &mut m,
            c + Vec3::Y * 0.5,
            Vec3::new(CAR_HALF_W + 0.04, 0.45, CAR_LEN * 0.5 - 0.8),
            glass,
        );
    }
    m
}

/// The bedrock behind the see-through ground: one dark plane `depth` metres under `y`.
pub fn backdrop_mesh(y: f32, half: f32) -> MeshData {
    let c = [0.10, 0.11, 0.14, 1.0];
    MeshData {
        positions: vec![
            [-half, y, -half],
            [half, y, -half],
            [-half, y, half],
            [half, y, half],
        ],
        normals: vec![[0.0, 1.0, 0.0]; 4],
        colors: vec![c; 4],
        indices: vec![0, 2, 1, 1, 2, 3],
    }
}

/// A train in the world: its entity keeps the last heading so that a train that stands keeps
/// pointing the way it came.
#[derive(Component)]
struct Train {
    color: u8,
    heading: Quat,
}

#[derive(Resource, Default)]
struct UndergroundRender {
    /// The input of the tunnel and ribbon meshes: what they were built from.
    key: Option<(usize, usize, u64, usize)>,
    ghost_key: Option<LineGhost>,
    tunnels: Option<Entity>,
    ribbons: Option<Entity>,
    ghost: Option<Entity>,
    backdrop: Option<Entity>,
    trains: Vec<Entity>,
    wall: Option<Handle<VertexColorMaterial>>,
    ribbon: Option<Handle<VertexColorMaterial>>,
    solid: Option<Handle<VertexColorMaterial>>,
    train_meshes: Vec<Option<Handle<Mesh>>>,
    was_on: bool,
}

fn platform_segments(t: &TransitView) -> BTreeSet<SegmentId> {
    t.net
        .stops
        .iter()
        .filter(|(_, s)| matches!(s.kind, cp_transit::StopKind::Station { .. }))
        .map(|(_, s)| s.kind.segment())
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn sync_underground(
    view: Option<Res<SimView>>,
    mode: Res<UndergroundView>,
    ghost: Res<LineGhost>,
    fraction: Res<TickFraction>,
    mut st: ResMut<UndergroundRender>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<VertexColorMaterial>>,
    mut trains: Query<(&mut Transform, &mut Train, &mut Mesh3d)>,
) {
    let on = mode.on();
    if !on {
        if st.was_on {
            for e in st
                .tunnels
                .take()
                .into_iter()
                .chain(st.ribbons.take())
                .chain(st.ghost.take())
                .chain(st.backdrop.take())
                .chain(st.trains.drain(..))
            {
                commands.entity(e).despawn();
            }
            st.key = None;
            st.ghost_key = None;
            st.was_on = false;
        }
        return;
    }
    st.was_on = true;
    let Some(view) = view else { return };
    let Some(content) = view.content.as_deref() else {
        return;
    };
    let wall = st
        .wall
        .get_or_insert_with(|| {
            materials.add(VertexColorMaterial::blended(0.7, 0.1).with_sort_bias(UNDER_SORT_BIAS))
        })
        .clone();
    let ribbon_mat = st
        .ribbon
        .get_or_insert_with(|| {
            materials.add(
                VertexColorMaterial::blended(0.9, 0.02)
                    .with_depth_bias(true)
                    .with_sort_bias(UNDER_SORT_BIAS),
            )
        })
        .clone();
    let solid = st
        .solid
        .get_or_insert_with(|| materials.add(VertexColorMaterial::opaque(0.5, 0.2)))
        .clone();

    // Tunnels and the ribbons of the lines: rebuilt when the net or the transit changed.
    let key = (
        Arc::as_ptr(&view.net).cast::<()>() as usize,
        Arc::as_ptr(&view.transit).cast::<()>() as usize,
        view.transit.plans_key,
        usize::from(st.tunnels.is_some()),
    );
    if st.key != Some(key) {
        st.key = Some(key);
        for e in st
            .tunnels
            .take()
            .into_iter()
            .chain(st.ribbons.take())
            .chain(st.backdrop.take())
        {
            commands.entity(e).despawn();
        }
        let floor = view
            .net
            .segments()
            .filter(|(_, s)| s.elevation == Elevation::Tunnel)
            .flat_map(|(_, s)| [s.curve.p0.y, s.curve.p3.y])
            .fold(f32::MAX, f32::min);
        let floor = if floor == f32::MAX { 0.0 } else { floor };
        st.backdrop = spawn_mesh(
            &mut commands,
            &mut meshes,
            solid.clone(),
            backdrop_mesh(floor - 20.0, cp_terrain::MAP_SIZE),
            "underground backdrop",
        );
        st.tunnels = spawn_mesh(
            &mut commands,
            &mut meshes,
            wall,
            tunnel_mesh(&view.net, content, &platform_segments(&view.transit)),
            "tunnels",
        );
        st.ribbons = spawn_mesh(
            &mut commands,
            &mut meshes,
            ribbon_mat.clone(),
            route_mesh(&view.transit),
            "metro lines",
        );
        // Spawned entities exist next frame: do not key on them again.
        st.key = Some((key.0, key.1, key.2, 1));
    }
    if st.ghost_key.as_ref() != Some(&*ghost) {
        st.ghost_key = Some(ghost.clone());
        if let Some(e) = st.ghost.take() {
            commands.entity(e).despawn();
        }
        st.ghost = spawn_mesh(
            &mut commands,
            &mut meshes,
            ribbon_mat,
            ghost_route_mesh(&ghost),
            "metro line ghost",
        );
    }

    // The trains: one entity each, moved every frame between two ticks.
    let want = &view.metro_trains;
    while st.trains.len() > want.len() {
        if let Some(e) = st.trains.pop() {
            commands.entity(e).despawn();
        }
    }
    let mesh_of = |st: &mut UndergroundRender, meshes: &mut Assets<Mesh>, color: u8| {
        let i = usize::from(color) % LINE_PALETTE_RGB.len();
        if st.train_meshes.len() <= i {
            st.train_meshes.resize(LINE_PALETTE_RGB.len(), None);
        }
        st.train_meshes[i]
            .get_or_insert_with(|| meshes.add(to_bevy_mesh(train_mesh(color))))
            .clone()
    };
    for (i, t) in want.iter().enumerate() {
        let at = t.pos.lerp(t.next, fraction.0.clamp(0.0, 1.0));
        let dir = t.next - t.pos;
        let flat = Vec3::new(dir.x, 0.0, dir.z);
        // The track axis is the car's floor line: lift it so the car sits in the tube.
        let place = |heading: Quat| {
            Transform::from_translation(at - Vec3::Y * (CAR_HALF_H + 0.3 - 1.0))
                .with_rotation(heading)
        };
        if let Some(&e) = st.trains.get(i)
            && let Ok((mut tf, mut train, mut mesh)) = trains.get_mut(e)
        {
            if flat.length_squared() > 1e-4 {
                train.heading = Quat::from_rotation_arc(Vec3::Z, flat.normalize());
            }
            *tf = place(train.heading);
            if train.color != t.color {
                train.color = t.color;
                mesh.0 = mesh_of(&mut st, &mut meshes, t.color);
            }
        } else if st.trains.len() == i {
            let heading = if flat.length_squared() > 1e-4 {
                Quat::from_rotation_arc(Vec3::Z, flat.normalize())
            } else {
                Quat::IDENTITY
            };
            let handle = mesh_of(&mut st, &mut meshes, t.color);
            let e = commands
                .spawn((
                    Mesh3d(handle),
                    MeshMaterial3d(solid.clone()),
                    place(heading),
                    Train {
                        color: t.color,
                        heading,
                    },
                    bevy::light::NotShadowCaster,
                    bevy::camera::visibility::NoFrustumCulling,
                    Name::new("metro train"),
                ))
                .id();
            st.trains.push(e);
        }
    }
}

/// Fades the ground, the roads and the buildings while the view is on and gives them back
/// when it is off. Their materials are shared, so a chunk made later is faded too: the pass
/// runs every frame while the view is on (and until nothing is left to give back after it).
#[allow(clippy::too_many_arguments)]
fn fade_surfaces(
    mode: Res<UndergroundView>,
    mut touched: Local<bool>,
    mut materials: ResMut<Assets<VertexColorMaterial>>,
    ground: Query<&MeshMaterial3d<VertexColorMaterial>, With<TerrainChunk>>,
    roads: Query<&MeshMaterial3d<VertexColorMaterial>, With<RoadChunk>>,
    buildings: Query<&MeshMaterial3d<VertexColorMaterial>, With<BuildingChunk>>,
    sea: Query<&MeshMaterial3d<VertexColorMaterial>, With<SeaPlane>>,
    water: Query<&MeshMaterial3d<VertexColorMaterial>, With<WaterSurface>>,
    layers: Option<Res<crate::PipeLayers>>,
) {
    let on = mode.on();
    // The water layer (BUG-109) shows the buildings see-through so the pipes under them read.
    // The building material has this one owner: the Underground view wins over the layer.
    let water_layer = layers.is_some_and(|l| l.visible);
    if !on && !water_layer && !*touched {
        return;
    }
    let mut changed = false;
    let mut pass = |handles: Vec<AssetId<VertexColorMaterial>>, on: bool, alpha: f32| {
        let want = if on { alpha } else { 1.0 };
        for id in handles {
            if materials
                .get(id)
                .is_some_and(|m| (m.fade.x - want).abs() > 1e-4)
                && let Some(mut m) = materials.get_mut(id)
            {
                m.set_see_through(on, alpha);
                changed = true;
            }
        }
    };
    let ids = |q: Vec<&MeshMaterial3d<VertexColorMaterial>>| {
        let mut v: Vec<_> = q.into_iter().map(|h| h.0.id()).collect();
        v.sort();
        v.dedup();
        v
    };
    pass(ids(ground.iter().collect()), on, GROUND_ALPHA);
    pass(ids(roads.iter().collect()), on, ROAD_ALPHA);
    let (b_on, b_alpha) = if on {
        (true, BUILDING_ALPHA)
    } else {
        (
            water_layer,
            crate::building_render::WATER_LAYER_BUILDING_ALPHA,
        )
    };
    pass(ids(buildings.iter().collect()), b_on, b_alpha);
    pass(ids(sea.iter().collect()), on, WATER_ALPHA);
    pass(ids(water.iter().collect()), on, WATER_ALPHA);
    *touched = on || water_layer || changed;
}

/// The View menu's Underground entry is the manual switch.
fn sync_manual(
    overlay: Option<Res<crate::overlay::OverlayState>>,
    mut mode: ResMut<UndergroundView>,
) {
    let want = overlay.is_some_and(|o| o.active == crate::overlay::Overlay::Underground);
    if mode.manual != want {
        mode.manual = want;
    }
}

pub struct UndergroundPlugin;

impl Plugin for UndergroundPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<UndergroundView>()
            .init_resource::<UndergroundRender>()
            .init_resource::<LineGhost>()
            .init_resource::<TickFraction>()
            .add_systems(
                Update,
                (sync_manual, sync_underground, fade_surfaces).chain(),
            );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// BUG-109: the building material has one owner. The water layer makes the buildings
    /// see-through, the Underground view keeps its own alpha over it, and both give it back.
    #[test]
    fn the_building_alpha_follows_the_view_and_the_water_layer() {
        let mut app = App::new();
        app.init_resource::<Assets<VertexColorMaterial>>()
            .init_resource::<UndergroundView>()
            .init_resource::<crate::PipeLayers>()
            .add_systems(Update, fade_surfaces);
        let handle = app
            .world_mut()
            .resource_mut::<Assets<VertexColorMaterial>>()
            .add(VertexColorMaterial::opaque(0.85, 0.05));
        app.world_mut().spawn((
            MeshMaterial3d(handle.clone()),
            crate::building_render::BuildingChunk::default(),
        ));
        let alpha = |app: &App| {
            app.world()
                .resource::<Assets<VertexColorMaterial>>()
                .get(&handle)
                .unwrap()
                .fade
                .x
        };
        let water = |app: &mut App, on: bool| {
            app.world_mut().resource_mut::<crate::PipeLayers>().visible = on;
        };
        let under = |app: &mut App, on: bool| {
            app.world_mut().resource_mut::<UndergroundView>().manual = on;
        };
        let settle = |app: &mut App| {
            for _ in 0..3 {
                app.update();
            }
        };
        settle(&mut app);
        assert_eq!(alpha(&app), 1.0);
        water(&mut app, true);
        settle(&mut app);
        assert_eq!(
            alpha(&app),
            crate::building_render::WATER_LAYER_BUILDING_ALPHA
        );
        // Underground on, water off: the view's alpha, stable over the updates.
        under(&mut app, true);
        water(&mut app, false);
        settle(&mut app);
        assert_eq!(alpha(&app), BUILDING_ALPHA);
        // Both on: the view wins.
        water(&mut app, true);
        settle(&mut app);
        assert_eq!(alpha(&app), BUILDING_ALPHA);
        under(&mut app, false);
        settle(&mut app);
        assert_eq!(
            alpha(&app),
            crate::building_render::WATER_LAYER_BUILDING_ALPHA
        );
        water(&mut app, false);
        settle(&mut app);
        assert_eq!(alpha(&app), 1.0);
    }

    fn finite(m: &MeshData) -> bool {
        m.positions
            .iter()
            .chain(&m.normals)
            .all(|p| p.iter().all(|c| c.is_finite()))
            && m.colors.iter().all(|c| c.iter().all(|x| x.is_finite()))
    }

    #[test]
    fn the_view_is_on_by_either_switch() {
        let mut v = UndergroundView::default();
        assert!(!v.on());
        v.auto = true;
        assert!(v.on());
        v.auto = false;
        v.manual = true;
        assert!(v.on());
    }

    /// A tunnel segment becomes a tube with finite numbers; a ground road adds nothing, and
    /// the platform is told apart by its colour.
    #[test]
    fn the_tunnel_mesh_has_a_tube_per_tunnel_segment_and_no_nan() {
        use cp_core::curve::CubicBezier3;
        let db = ContentDb::load_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content"))
            .unwrap();
        let (metro, street) = (
            db.road_id("metro_track").unwrap(),
            db.road_id("basic_2l").unwrap(),
        );
        let mut net = NetGraph::default();
        let line =
            |a: f32, b: f32, y: f32| CubicBezier3::line(Vec3::new(a, y, 0.0), Vec3::new(b, y, 0.0));
        let tube = net
            .add_segment(
                db.road(metro),
                metro,
                line(0.0, 96.0, -12.0),
                Elevation::Tunnel,
            )
            .unwrap();
        net.add_segment(
            db.road(street),
            street,
            line(0.0, 96.0, 0.0),
            Elevation::Ground,
        )
        .unwrap();
        let plain = tunnel_mesh(&net, &db, &BTreeSet::new());
        assert!(finite(&plain) && !plain.indices.is_empty());
        assert!(
            plain
                .indices
                .iter()
                .all(|&i| (i as usize) < plain.positions.len())
        );
        // Every vertex hangs around the tunnel axis: nothing of the street is in it.
        assert!(plain.positions.iter().all(|p| p[1] < -12.0 + 6.0));
        let platform = tunnel_mesh(&net, &db, &BTreeSet::from([tube]));
        assert_eq!(platform.positions.len(), plain.positions.len());
        assert_ne!(platform.colors[0], plain.colors[0]);
    }

    #[test]
    fn a_train_is_three_clean_cars_on_the_track_axis() {
        for color in [0u8, 5, 11, 200] {
            let m = train_mesh(color);
            assert!(finite(&m) && m.triangle_count() == CARS * 2 * 12);
            assert!(m.indices.iter().all(|&i| (i as usize) < m.positions.len()));
            // Unit normals, and every car lies on the +Z axis within its length.
            for n in &m.normals {
                let l = Vec3::from(*n).length();
                assert!((l - 1.0).abs() < 1e-4, "{n:?}");
            }
            let len = CARS as f32 * CAR_LEN + (CARS as f32 - 1.0) * CAR_GAP;
            assert!(m.positions.iter().all(|p| p[2].abs() <= len * 0.5 + 1e-3));
        }
    }

    #[test]
    fn a_ribbon_on_the_track_has_a_quad_per_span_and_no_nan() {
        let pts: Vec<Vec2> = (0..6).map(|i| Vec2::new(i as f32 * 4.0, 10.0)).collect();
        let heights = vec![-12.0; pts.len()];
        let mut m = MeshData::default();
        ribbon3(&mut m, &pts, &heights, 1.3, RIBBON_LIFT, [1.0; 4]);
        assert_eq!(m.indices.len(), 5 * 6);
        assert!(finite(&m));
        for tri in m.indices.chunks(3) {
            let [a, b, c] = [tri[0], tri[1], tri[2]].map(|i| Vec3::from(m.positions[i as usize]));
            assert!((b - a).cross(c - a).y > 0.0, "faces up");
        }
    }
}
