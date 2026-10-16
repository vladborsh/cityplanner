//! Power lines and pylons in the world (`architecture/system-design.md` §13.3): a pylon at every
//! node of the power network and two sagging wires along every link. Pipes (T-103, §13.5) are
//! 3D tubes with a translucent 40 m coverage band, drawn while a water tool or the water
//! overlay is active ([`PipeLayers`]).
//!
//! The geometry is a pure function of the [`UtilityGraph`] ([`power_mesh`]); the Bevy side
//! keeps one mesh entity and rebuilds it when the graph snapshot (an `Arc` in the view) changes.

use std::sync::Arc;

use bevy::asset::RenderAssetUsages;
use bevy::camera::primitives::Aabb;
use bevy::light::NotShadowCaster;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use cp_core::curve::{CubicBezier3, to_xz};
use cp_sim::SimView;
use cp_terrain::Terrain;
use cp_utility::{UtilityGraph, UtilityNet};

use crate::chunk::{MeshData, terrain_mesh_height};
use crate::material::VertexColorMaterial;

/// Height of a pylon, metres.
pub const PYLON_HEIGHT: f32 = 16.0;
/// Half length of the crossarm, metres.
pub const ARM_HALF: f32 = 3.0;
/// Wire sag at mid-span as a fraction of the span length.
const SAG: f32 = 0.015;
const WIRE_STEPS: usize = 8;
const PYLON_COLOR: [f32; 4] = [0.52, 0.54, 0.58, 1.0];
const WIRE_COLOR: [f32; 4] = [0.12, 0.12, 0.14, 1.0];

/// A square prism of side `t` from `a` to `b` (side faces only), flat-shaded.
fn prism(m: &mut MeshData, a: Vec3, b: Vec3, t: f32, color: [f32; 4]) {
    let Some(d) = (b - a).try_normalize() else {
        return;
    };
    let up = if d.y.abs() > 0.9 { Vec3::X } else { Vec3::Y };
    let s = d.cross(up).normalize() * (t * 0.5);
    let v = s.cross(d).normalize() * (t * 0.5);
    let ring = [s + v, -s + v, -s - v, s - v];
    for i in 0..4 {
        let (p, q) = (ring[i], ring[(i + 1) % 4]);
        let n = (p + q).normalize_or_zero().to_array();
        let base = m.positions.len() as u32;
        for c in [a + p, a + q, b + q, b + p] {
            m.positions.push(c.to_array());
            m.normals.push(n);
            m.colors.push(color);
        }
        // Both windings: the prism is thin and seen from any side.
        m.indices
            .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        m.indices
            .extend_from_slice(&[base, base + 2, base + 1, base, base + 3, base + 2]);
    }
}

/// The crossarm direction of a node: horizontal, perpendicular to its first link.
fn arm_dir(graph: &UtilityGraph, node: &cp_utility::UtilityNode) -> Vec3 {
    let along = node
        .links
        .first()
        .and_then(|l| graph.link(*l))
        .map(|l| {
            let t = if graph.node(l.a).is_some_and(|n| n.pos == node.pos) {
                0.0
            } else {
                1.0
            };
            l.curve.tangent(t)
        })
        .unwrap_or(Vec3::X);
    Vec3::new(along.z, 0.0, -along.x)
        .try_normalize()
        .unwrap_or(Vec3::Z)
}

/// Wire attachment points (left, right) at the top of the pylon on `node`.
fn tips(graph: &UtilityGraph, node: &cp_utility::UtilityNode) -> [Vec3; 2] {
    let top = node.pos + Vec3::Y * PYLON_HEIGHT;
    let arm = arm_dir(graph, node) * ARM_HALF;
    [top + arm, top - arm]
}

/// Pylons and wires of the whole power network.
pub fn power_mesh(graph: &UtilityGraph) -> MeshData {
    power_mesh_colored(graph, PYLON_COLOR, WIRE_COLOR)
}

/// [`power_mesh`] with explicit colours (the ghost and the highlight).
pub fn power_mesh_colored(
    graph: &UtilityGraph,
    pylon_color: [f32; 4],
    wire_color: [f32; 4],
) -> MeshData {
    let mut m = MeshData::default();
    for (_, n) in graph.nodes().filter(|(_, n)| n.net == UtilityNet::Power) {
        let top = n.pos + Vec3::Y * PYLON_HEIGHT;
        prism(&mut m, n.pos, top, 1.0, pylon_color);
        let [l, r] = tips(graph, n);
        prism(&mut m, l, r, 0.5, pylon_color);
    }
    for (_, link) in graph.links().filter(|(_, l)| l.net == UtilityNet::Power) {
        let (Some(a), Some(b)) = (graph.node(link.a), graph.node(link.b)) else {
            continue;
        };
        let (ta, mut tb) = (tips(graph, a), tips(graph, b));
        // Pair the tips so the two wires do not cross.
        if ta[0].distance(tb[0]) + ta[1].distance(tb[1])
            > ta[0].distance(tb[1]) + ta[1].distance(tb[0])
        {
            tb.swap(0, 1);
        }
        for (p, q) in ta.into_iter().zip(tb) {
            let sag = SAG * p.distance(q);
            let at = |i: usize| {
                let t = i as f32 / WIRE_STEPS as f32;
                p.lerp(q, t) - Vec3::Y * (sag * 4.0 * t * (1.0 - t))
            };
            for i in 0..WIRE_STEPS {
                prism(&mut m, at(i), at(i + 1), 0.15, wire_color);
            }
        }
    }
    m
}

/// The power graph of a ghost: one node per distinct end point of `segments`, one link per
/// segment (straight). What the power tool would build, or the raw path when it is invalid.
pub fn ghost_graph(segments: &[[Vec3; 2]]) -> UtilityGraph {
    let mut g = UtilityGraph::new();
    let mut ids: Vec<(Vec3, cp_utility::UtilityNodeId)> = Vec::new();
    let mut node =
        |g: &mut UtilityGraph, p: Vec3| match ids.iter().find(|(q, _)| q.distance(p) < 0.05) {
            Some((_, id)) => *id,
            None => {
                let id = g.add_node(UtilityNet::Power, p);
                ids.push((p, id));
                id
            }
        };
    for [a, b] in segments {
        let (na, nb) = (node(&mut g, *a), node(&mut g, *b));
        // A degenerate segment (same node twice) is simply not a link.
        let _ = g.add_link(na, nb, CubicBezier3::line(*a, *b));
    }
    g
}

// ---------------------------------------------------------------------------------------
// Pipes (T-103)

/// Radius of a pipe, metres.
pub const PIPE_RADIUS: f32 = 0.6;
/// Height of the pipe axis above the rendered terrain, metres.
pub const PIPE_LIFT: f32 = 0.7;
/// Radius of the coupling at every node, metres.
pub const SLEEVE_RADIUS: f32 = 0.8;
/// Length of a coupling along the pipe, metres.
const SLEEVE_LENGTH: f32 = 1.0;
/// Faces around a pipe.
const PIPE_FACES: usize = 8;
/// Ring spacing along a pipe and a band, metres.
pub const RING_STEP: f32 = 4.0;
/// Height of the coverage band above the rendered terrain, metres.
pub const BAND_LIFT: f32 = 0.2;
/// Segments of a half-disc cap per 180 degrees.
const CAP_STEPS: usize = 12;
pub const PIPE_COLOR: [f32; 4] = [0.32, 0.50, 0.72, 1.0];
pub const SLEEVE_COLOR: [f32; 4] = [0.22, 0.34, 0.52, 1.0];
/// Coverage band of the built pipes.
pub const BAND_COLOR: [f32; 4] = [0.35, 0.65, 0.95, 0.15];
pub const GHOST_OK: [f32; 3] = [0.2, 0.9, 0.3];
pub const GHOST_BAD: [f32; 3] = [0.95, 0.2, 0.2];

/// Ring positions (`t` of the curve) every [`RING_STEP`] metres of arc length, ends included.
fn ring_params(c: &CubicBezier3) -> Vec<f32> {
    let table = c.arc_table();
    let len = table.length();
    let n = ((len / RING_STEP).ceil() as usize).max(1);
    (0..=n)
        .map(|i| table.s_to_t(len * i as f32 / n as f32))
        .collect()
}

/// A tube around `axis` points (already lifted), with smooth normals; appended to `m`.
fn tube(m: &mut MeshData, rings: &[(Vec3, Vec3)], radius: f32, color: [f32; 4]) {
    let base = m.positions.len() as u32;
    for &(c, t) in rings {
        let t = t.try_normalize().unwrap_or(Vec3::X);
        let side = Vec3::Y
            .cross(t)
            .try_normalize()
            .unwrap_or_else(|| Vec3::Z.cross(t).normalize_or_zero());
        let up = t.cross(side).normalize_or_zero();
        for k in 0..PIPE_FACES {
            let a = k as f32 / PIPE_FACES as f32 * std::f32::consts::TAU;
            let n = side * a.cos() + up * a.sin();
            m.positions.push((c + n * radius).to_array());
            m.normals.push(n.to_array());
            m.colors.push(color);
        }
    }
    let f = PIPE_FACES as u32;
    for r in 0..rings.len().saturating_sub(1) as u32 {
        for k in 0..f {
            let (a, b) = (base + r * f + k, base + r * f + (k + 1) % f);
            let (c, d) = (a + f, b + f);
            m.indices.extend_from_slice(&[a, c, b, b, c, d]);
        }
    }
}

/// The 3D pipes along `curves`: a tube of [`PIPE_RADIUS`] with 8 faces, a ring every
/// [`RING_STEP`] metres, its axis [`PIPE_LIFT`] above the rendered terrain, and a coupling of
/// [`SLEEVE_RADIUS`] at both ends of every curve. The tube rings come first.
pub fn pipe_mesh(curves: &[CubicBezier3], terrain: &Terrain) -> MeshData {
    pipe_mesh_colored(curves, terrain, PIPE_COLOR, SLEEVE_COLOR)
}

/// [`pipe_mesh`] with explicit colours (the ghost and the highlight).
pub fn pipe_mesh_colored(
    curves: &[CubicBezier3],
    terrain: &Terrain,
    color: [f32; 4],
    sleeve: [f32; 4],
) -> MeshData {
    let mut m = MeshData::default();
    let lift = |p: Vec3| {
        Vec3::new(
            p.x,
            terrain_mesh_height(terrain, Vec2::new(p.x, p.z)) + PIPE_LIFT,
            p.z,
        )
    };
    let mut ends: Vec<(Vec3, Vec3)> = Vec::new();
    for c in curves {
        let pts: Vec<Vec3> = ring_params(c).iter().map(|&t| lift(c.point(t))).collect();
        // Tangent from the lifted axis itself, so the rings follow the terrain.
        let rings: Vec<(Vec3, Vec3)> = (0..pts.len())
            .map(|i| {
                let (a, b) = (pts[i.saturating_sub(1)], pts[(i + 1).min(pts.len() - 1)]);
                (pts[i], b - a)
            })
            .collect();
        tube(&mut m, &rings, PIPE_RADIUS, color);
        ends.push(rings[0]);
        ends.push(rings[rings.len() - 1]);
    }
    for (p, t) in ends {
        let t = t.try_normalize().unwrap_or(Vec3::X);
        let half = t * (SLEEVE_LENGTH * 0.5);
        tube(
            &mut m,
            &[(p - half, t), (p + half, t)],
            SLEEVE_RADIUS,
            sleeve,
        );
    }
    m
}

/// A flat translucent band of half width `half_width` along `curves` (rings every
/// [`RING_STEP`] metres, [`BAND_LIFT`] above the rendered terrain), with half-disc caps of
/// radius `half_width` where no link continues: at the ends and the outer side of bends. One
/// mesh for all curves.
pub fn coverage_band_mesh(
    curves: &[CubicBezier3],
    half_width: f32,
    terrain: &Terrain,
    color: [f32; 4],
) -> MeshData {
    let mut m = MeshData::default();
    let y = |p: Vec2| terrain_mesh_height(terrain, p) + BAND_LIFT;
    let push = |m: &mut MeshData, p: Vec2| {
        m.positions.push([p.x, y(p), p.y]);
        m.normals.push([0.0, 1.0, 0.0]);
        m.colors.push(color);
        (m.positions.len() - 1) as u32
    };
    // Outgoing directions at every distinct end point (XZ).
    let mut nodes: Vec<(Vec2, Vec<f32>)> = Vec::new();
    let mut note = |p: Vec2, dir: Vec2| {
        let a = dir.y.atan2(dir.x);
        match nodes.iter_mut().find(|(q, _)| q.distance(p) < 0.05) {
            Some((_, dirs)) => dirs.push(a),
            None => nodes.push((p, vec![a])),
        }
    };
    for c in curves {
        let (t0, t1) = (c.tangent(0.0), c.tangent(1.0));
        note(to_xz(c.p0), Vec2::new(t0.x, t0.z));
        note(to_xz(c.p3), Vec2::new(-t1.x, -t1.z));
        let params = ring_params(c);
        let mut prev: Option<[u32; 2]> = None;
        for t in params {
            let p = to_xz(c.point(t));
            let tg = c.tangent(t);
            let side = Vec2::new(tg.z, -tg.x).try_normalize().unwrap_or(Vec2::Y) * half_width;
            let cur = [push(&mut m, p + side), push(&mut m, p - side)];
            if let Some([pl, pr]) = prev {
                m.indices
                    .extend_from_slice(&[pl, pr, cur[1], pl, cur[1], cur[0]]);
            }
            prev = Some(cur);
        }
    }
    // Caps: the arcs of the disc around a node that no link's band covers (no link direction
    // within 90 degrees).
    use std::f32::consts::{FRAC_PI_2, PI, TAU};
    for (p, mut dirs) in nodes {
        dirs.sort_by(f32::total_cmp);
        let n = dirs.len();
        for i in 0..n {
            let (a, b) = (
                dirs[i],
                dirs[(i + 1) % n] + if i + 1 == n { TAU } else { 0.0 },
            );
            let gap = if n == 1 { TAU } else { b - a };
            if gap <= PI + 1e-4 {
                continue;
            }
            let (from, to) = (
                a + FRAC_PI_2,
                if n == 1 {
                    a + TAU - FRAC_PI_2
                } else {
                    b - FRAC_PI_2
                },
            );
            let steps = (((to - from) / PI * CAP_STEPS as f32).ceil() as usize).max(2);
            let centre = push(&mut m, p);
            let mut prev = None;
            for s in 0..=steps {
                let ang = from + (to - from) * s as f32 / steps as f32;
                let v = push(&mut m, p + Vec2::new(ang.cos(), ang.sin()) * half_width);
                if let Some(pv) = prev {
                    // Counter-clockwise in (x, z) with z down faces up when wound (c, pv, v)
                    // reversed; fix by the sign of the cross product below.
                    let (pa, pb) = (
                        Vec2::from_array([
                            m.positions[pv as usize][0],
                            m.positions[pv as usize][2],
                        ]),
                        Vec2::from_array([m.positions[v as usize][0], m.positions[v as usize][2]]),
                    );
                    if (pa - p).perp_dot(pb - p) > 0.0 {
                        m.indices.extend_from_slice(&[centre, v, pv]);
                    } else {
                        m.indices.extend_from_slice(&[centre, pv, v]);
                    }
                }
                prev = Some(v);
            }
        }
    }
    m
}

/// The two meshes of the built pipes (pipes, coverage band), or `None` when pipes are not
/// shown (the band is only built while a water tool or the water overlay is active).
pub fn built_pipe_meshes(
    graph: &UtilityGraph,
    terrain: &Terrain,
    visible: bool,
    reach: f32,
) -> Option<(MeshData, MeshData)> {
    if !visible {
        return None;
    }
    let curves: Vec<CubicBezier3> = graph
        .links()
        .filter(|(_, l)| l.net == UtilityNet::Pipe)
        .map(|(_, l)| l.curve)
        .collect();
    Some((
        pipe_mesh(&curves, terrain),
        coverage_band_mesh(&curves, reach, terrain, BAND_COLOR),
    ))
}

/// What the tools want drawn of the pipe network (`cp_tools` writes it every frame).
#[derive(Resource, Default, Clone, Debug, PartialEq)]
pub struct PipeLayers {
    /// A water tool, the pump / outflow placement or the water overlay is active.
    pub visible: bool,
    /// The pipe the cursor would build: straight pieces and whether the plan is valid.
    pub ghost: Option<PipeGhost>,
    /// The piece the remove tool would delete.
    pub highlight: Option<CubicBezier3>,
    /// The power line the cursor would build (pylons and wires, translucent). Drawn whether or
    /// not pipes are visible.
    pub power_ghost: Option<PowerGhost>,
    /// The power link the remove tool would delete, as its end points (drawn red).
    pub power_highlight: Option<[Vec3; 2]>,
}

/// The straight pieces of a power-line plan and whether the plan is valid.
#[derive(Clone, Debug, PartialEq)]
pub struct PowerGhost {
    pub segments: Vec<[Vec3; 2]>,
    pub valid: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PipeGhost {
    pub curves: Vec<CubicBezier3>,
    pub valid: bool,
}

#[derive(Resource, Default)]
struct PipeRender {
    graph: Option<Arc<UtilityGraph>>,
    terrain: usize,
    shown: Option<PipeLayers>,
    built: Vec<Entity>,
    ghost: Vec<Entity>,
    opaque: Option<Handle<VertexColorMaterial>>,
    blended: Option<Handle<VertexColorMaterial>>,
}

fn spawn_mesh(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    material: Handle<VertexColorMaterial>,
    data: MeshData,
    name: &str,
) -> Option<Entity> {
    if data.indices.is_empty() {
        return None;
    }
    let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    for p in &data.positions {
        lo = lo.min(Vec3::from(*p));
        hi = hi.max(Vec3::from(*p));
    }
    let mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, data.positions)
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, data.normals)
    .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, data.colors)
    .with_inserted_indices(Indices::U32(data.indices));
    Some(
        commands
            .spawn((
                Mesh3d(meshes.add(mesh)),
                MeshMaterial3d(material),
                Transform::default(),
                Aabb::from_min_max(lo, hi),
                NotShadowCaster,
                Name::new(name.to_string()),
            ))
            .id(),
    )
}

/// Builds the pipe entities: the network (rebuilt when its snapshot changes, only while
/// visible), and the ghost and highlight (rebuilt when they change).
fn sync_pipes(
    view: Option<Res<SimView>>,
    layers: Res<PipeLayers>,
    mut state: ResMut<PipeRender>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<VertexColorMaterial>>,
) {
    let Some(view) = view else { return };
    let Some(terrain) = view.terrain.as_deref() else {
        return;
    };
    let opaque = state
        .opaque
        .get_or_insert_with(|| materials.add(VertexColorMaterial::opaque(0.6, 0.1)))
        .clone();
    let blended = state
        .blended
        .get_or_insert_with(|| {
            materials.add(VertexColorMaterial::blended(0.9, 0.02).with_depth_bias(true))
        })
        .clone();
    let tkey = std::ptr::from_ref(terrain) as usize;
    let st = &mut *state;
    // The pipe reach of the content (`balance.ron` `networks`), the half width of the band.
    let reach = view
        .content
        .as_deref()
        .map_or(cp_content::Networks::default().pipe_reach_m, |db| {
            db.balance().networks.pipe_reach_m
        });

    // The built network.
    let stale = !st
        .graph
        .as_ref()
        .is_some_and(|g| Arc::ptr_eq(g, &view.utilities))
        || st.terrain != tkey;
    if !layers.visible {
        for e in st.built.drain(..) {
            commands.entity(e).despawn();
        }
        st.graph = None;
    } else if stale {
        for e in st.built.drain(..) {
            commands.entity(e).despawn();
        }
        st.graph = Some(Arc::clone(&view.utilities));
        st.terrain = tkey;
        if let Some((pipes, band)) = built_pipe_meshes(&view.utilities, terrain, true, reach) {
            st.built.extend(spawn_mesh(
                &mut commands,
                &mut meshes,
                opaque.clone(),
                pipes,
                "pipes",
            ));
            st.built.extend(spawn_mesh(
                &mut commands,
                &mut meshes,
                blended.clone(),
                band,
                "pipe coverage",
            ));
        }
    }

    // The ghost and the highlight.
    let want = PipeLayers {
        visible: layers.visible,
        ghost: layers.ghost.clone().filter(|_| layers.visible),
        highlight: layers.highlight.filter(|_| layers.visible),
        power_ghost: layers.power_ghost.clone(),
        power_highlight: layers.power_highlight,
    };
    if st.shown.as_ref() == Some(&want) {
        return;
    }
    for e in st.ghost.drain(..) {
        commands.entity(e).despawn();
    }
    if let Some(g) = &want.ghost {
        let rgb = if g.valid { GHOST_OK } else { GHOST_BAD };
        let tube_c = [rgb[0], rgb[1], rgb[2], 0.65];
        st.ghost.extend(spawn_mesh(
            &mut commands,
            &mut meshes,
            blended.clone(),
            pipe_mesh_colored(&g.curves, terrain, tube_c, tube_c),
            "pipe ghost",
        ));
        st.ghost.extend(spawn_mesh(
            &mut commands,
            &mut meshes,
            blended.clone(),
            coverage_band_mesh(&g.curves, reach, terrain, [rgb[0], rgb[1], rgb[2], 0.18]),
            "pipe ghost coverage",
        ));
    }
    if let Some(c) = want.highlight {
        let red = [1.0, 0.45, 0.1, 0.9];
        st.ghost.extend(spawn_mesh(
            &mut commands,
            &mut meshes,
            blended.clone(),
            pipe_mesh_colored(&[c], terrain, red, red),
            "pipe highlight",
        ));
    }
    if let Some(g) = &want.power_ghost {
        let rgb = if g.valid { GHOST_OK } else { GHOST_BAD };
        let c = [rgb[0], rgb[1], rgb[2], 0.55];
        st.ghost.extend(spawn_mesh(
            &mut commands,
            &mut meshes,
            blended.clone(),
            power_mesh_colored(&ghost_graph(&g.segments), c, c),
            "power ghost",
        ));
    }
    if let Some(seg) = want.power_highlight {
        let red = [GHOST_BAD[0], GHOST_BAD[1], GHOST_BAD[2], 0.8];
        st.ghost.extend(spawn_mesh(
            &mut commands,
            &mut meshes,
            blended,
            power_mesh_colored(&ghost_graph(&[seg]), red, red),
            "power highlight",
        ));
    }
    st.shown = Some(want);
}

pub struct UtilityRenderPlugin;

impl Plugin for UtilityRenderPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PowerRender>()
            .init_resource::<PipeLayers>()
            .init_resource::<PipeRender>()
            .add_systems(Update, (sync_power, sync_pipes));
    }
}

#[derive(Resource, Default)]
struct PowerRender {
    last: Option<Arc<UtilityGraph>>,
    entity: Option<Entity>,
    material: Option<Handle<VertexColorMaterial>>,
}

fn sync_power(
    view: Option<Res<SimView>>,
    mut state: ResMut<PowerRender>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<VertexColorMaterial>>,
) {
    let Some(view) = view else { return };
    if state
        .last
        .as_ref()
        .is_some_and(|g| Arc::ptr_eq(g, &view.utilities))
    {
        return;
    }
    state.last = Some(Arc::clone(&view.utilities));
    if let Some(e) = state.entity.take() {
        commands.entity(e).despawn();
    }
    let data = power_mesh(&view.utilities);
    if data.indices.is_empty() {
        return;
    }
    let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    for p in &data.positions {
        lo = lo.min(Vec3::from(*p));
        hi = hi.max(Vec3::from(*p));
    }
    let material = state
        .material
        .get_or_insert_with(|| materials.add(VertexColorMaterial::opaque(0.8, 0.1)))
        .clone();
    let mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, data.positions)
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, data.normals)
    .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, data.colors)
    .with_inserted_indices(Indices::U32(data.indices));
    state.entity = Some(
        commands
            .spawn((
                Mesh3d(meshes.add(mesh)),
                MeshMaterial3d(material),
                Transform::default(),
                Aabb::from_min_max(lo, hi),
                NotShadowCaster,
                Name::new("power lines"),
            ))
            .id(),
    );
}

#[cfg(test)]
mod tests {
    const BUILDING_REACH: f32 = 40.0;
    use super::*;
    use bevy::math::Vec3;
    use cp_core::curve::CubicBezier3;
    use cp_terrain::Terrain;

    fn line_graph() -> UtilityGraph {
        let mut g = UtilityGraph::new();
        let a = g.add_node(UtilityNet::Power, Vec3::new(100.0, 5.0, 100.0));
        let b = g.add_node(UtilityNet::Power, Vec3::new(180.0, 5.0, 100.0));
        g.add_link(
            a,
            b,
            CubicBezier3::line(Vec3::new(100.0, 5.0, 100.0), Vec3::new(180.0, 5.0, 100.0)),
        )
        .unwrap();
        // A pipe next to it is not drawn in the world.
        let p = g.add_node(UtilityNet::Pipe, Vec3::new(100.0, 5.0, 300.0));
        let q = g.add_node(UtilityNet::Pipe, Vec3::new(180.0, 5.0, 300.0));
        g.add_link(
            p,
            q,
            CubicBezier3::line(Vec3::new(100.0, 5.0, 300.0), Vec3::new(180.0, 5.0, 300.0)),
        )
        .unwrap();
        g
    }

    #[test]
    fn power_ghost_has_a_pylon_per_plan_node_and_wires_per_link() {
        let (a, b, c) = (
            Vec3::new(0.0, 2.0, 0.0),
            Vec3::new(60.0, 2.0, 0.0),
            Vec3::new(60.0, 2.0, 60.0),
        );
        // Two links sharing node `b`: three nodes, two links.
        let g = ghost_graph(&[[a, b], [b, c]]);
        assert_eq!(g.node_count(), 3);
        assert_eq!(g.link_count(), 2);
        let tint = [0.2, 0.9, 0.3, 0.55];
        let m = power_mesh_colored(&g, tint, tint);
        assert!(m.triangle_count() > 0);
        assert!(m.colors.iter().all(|c| *c == tint), "one translucent tint");
        // Every node has a pylon: geometry reaches pylon height above each one.
        for p in [a, b, c] {
            assert!(
                m.positions.iter().any(|q| (q[0] - p.x).abs() < 2.0
                    && (q[2] - p.z).abs() < 2.0
                    && (q[1] - (p.y + PYLON_HEIGHT)).abs() < 1.0),
                "no pylon at {p}"
            );
        }
    }

    #[test]
    fn power_mesh_has_pylons_and_sagging_wires_and_no_pipes() {
        let m = power_mesh(&line_graph());
        assert!(m.triangle_count() > 0);
        assert!(m.positions.iter().all(|p| p[2] < 200.0), "no pipe geometry");
        // Pylons rise to PYLON_HEIGHT above the node; wires hang below the tips mid-span.
        let top = m.positions.iter().map(|p| p[1]).fold(f32::MIN, f32::max);
        assert!((top - (5.0 + PYLON_HEIGHT)).abs() < 1.0, "{top}");
        let mid_low = m
            .positions
            .iter()
            .filter(|p| (p[0] - 140.0).abs() < 6.0)
            .map(|p| p[1])
            .fold(f32::MAX, f32::min);
        assert!(mid_low < 5.0 + PYLON_HEIGHT - 0.5, "wire sags: {mid_low}");
        assert!(mid_low > 5.0 + 3.0, "but stays well above the ground");
        assert_eq!(m.positions.len(), m.normals.len());
        assert_eq!(m.positions.len(), m.colors.len());
    }

    fn slope() -> Terrain {
        use cp_terrain::{GRID, MAX_HEIGHT, MapMeta};
        let meta = MapMeta {
            name: "t".into(),
            sea_level: 0.0,
            start_tile: (2, 2),
            external_connections: vec![],
            entry_road: None,
        };
        let h: Vec<u16> = (0..GRID * GRID)
            .map(|k| {
                let (x, z) = (k % GRID, k / GRID);
                ((30.0 + x as f32 * 0.4 + (z % 3) as f32 * 0.3) / MAX_HEIGHT * 65535.0) as u16
            })
            .collect();
        Terrain::from_samples(meta, &h, &vec![0; GRID * GRID]).unwrap()
    }

    fn axis(x0: f32, z0: f32, x1: f32, z1: f32) -> CubicBezier3 {
        CubicBezier3::line(Vec3::new(x0, 0.0, z0), Vec3::new(x1, 0.0, z1))
    }

    #[test]
    fn pipe_mesh_has_a_ring_every_4_m_on_the_terrain_and_unit_normals() {
        let t = slope();
        assert!(pipe_mesh(&[], &t).indices.is_empty());
        let m = pipe_mesh(&[axis(1000.0, 1000.0, 1080.0, 1000.0)], &t);
        let body = 21 * PIPE_FACES;
        // 21 rings of 8, then two couplings of two rings each.
        assert_eq!(m.positions.len(), body + 2 * 2 * PIPE_FACES);
        for n in &m.normals {
            let l = Vec3::from(*n).length();
            assert!((l - 1.0).abs() < 1e-4, "{n:?}");
        }
        assert!(m.positions.iter().flatten().all(|v| v.is_finite()));
        for ring in m.positions[..body].chunks(PIPE_FACES) {
            let c = ring.iter().map(|p| Vec3::from(*p)).sum::<Vec3>() / PIPE_FACES as f32;
            let ground = terrain_mesh_height(&t, Vec2::new(c.x, c.z));
            assert!((c.y - ground - PIPE_LIFT).abs() < 0.01, "{c} vs {ground}");
            for p in ring {
                let r = (Vec3::from(*p) - c).length();
                assert!((r - PIPE_RADIUS).abs() < 1e-3);
            }
        }
        // Rings are 4 m apart along the pipe.
        let x = |ring: usize| m.positions[ring * PIPE_FACES][0];
        assert!(((x(1) - x(0)) - 4.0).abs() < 0.05);
        // The couplings are wider than the pipe.
        let wide = m.positions[body..].iter().any(|p| {
            (Vec3::from(*p) - Vec3::new(1000.0, 0.0, 1000.0))
                .xz()
                .length()
                < 1.0
        });
        assert!(wide);
    }

    use bevy::math::Vec2;

    fn dist_to_segment(p: Vec2, a: Vec2, b: Vec2) -> f32 {
        let ab = b - a;
        let t = ((p - a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0);
        p.distance(a + ab * t)
    }

    #[test]
    fn the_coverage_band_follows_the_pipe_and_is_as_wide_as_the_reach() {
        let t = slope();
        assert!(
            coverage_band_mesh(&[], BUILDING_REACH, &t, BAND_COLOR)
                .indices
                .is_empty()
        );
        let (a, b) = (Vec2::new(1000.0, 1000.0), Vec2::new(1080.0, 1000.0));
        let m = coverage_band_mesh(&[axis(a.x, a.y, b.x, b.y)], BUILDING_REACH, &t, BAND_COLOR);
        let pts: Vec<Vec2> = m.positions.iter().map(|p| Vec2::new(p[0], p[2])).collect();
        for p in &pts {
            assert!(dist_to_segment(*p, a, b) <= BUILDING_REACH + 0.01, "{p}");
        }
        // The two outer vertices of every ring are BUILDING_REACH either side of the axis,
        // and each ring is centred on the pipe.
        for ring in pts[..21 * 2].chunks(2) {
            assert!(((ring[0].y - ring[1].y).abs() - 2.0 * BUILDING_REACH).abs() < 0.01);
            let centre = (ring[0] + ring[1]) * 0.5;
            assert!(dist_to_segment(centre, a, b) < 0.01);
        }
        // The caps cover a point 39 m beyond either end along the axis.
        let covers = |q: Vec2| {
            m.indices.chunks(3).any(|tri| {
                let [p0, p1, p2] = [tri[0], tri[1], tri[2]].map(|i| pts[i as usize]);
                let s = |a: Vec2, b: Vec2, c: Vec2| (b - a).perp_dot(c - a);
                let (d0, d1, d2) = (s(p0, p1, q), s(p1, p2, q), s(p2, p0, q));
                (d0 >= -1e-3 && d1 >= -1e-3 && d2 >= -1e-3)
                    || (d0 <= 1e-3 && d1 <= 1e-3 && d2 <= 1e-3)
            })
        };
        assert!(covers(Vec2::new(b.x + BUILDING_REACH - 1.0, b.y)));
        assert!(covers(Vec2::new(a.x - BUILDING_REACH + 1.0, a.y)));
        assert!(!covers(Vec2::new(b.x + BUILDING_REACH + 2.0, b.y)));
        // Flat, a little above the rendered terrain, facing up.
        for p in &m.positions {
            let g = terrain_mesh_height(&t, Vec2::new(p[0], p[2]));
            assert!((p[1] - g - BAND_LIFT).abs() < 1e-3);
        }
        for tri in m.indices.chunks(3) {
            let [a, b, c] = [tri[0], tri[1], tri[2]].map(|i| Vec3::from(m.positions[i as usize]));
            assert!((b - a).cross(c - a).y > 0.0, "a triangle faces down");
        }
    }

    #[test]
    fn the_band_turns_the_corner_of_a_pipe_with_a_90_degree_bend() {
        let t = slope();
        let c = Vec3::new(1000.0, 0.0, 1000.0);
        let curves = [
            axis(940.0, 1000.0, 1000.0, 1000.0),
            axis(1000.0, 1000.0, 1000.0, 1060.0),
        ];
        let m = coverage_band_mesh(&curves, BUILDING_REACH, &t, BAND_COLOR);
        let pts: Vec<Vec2> = m.positions.iter().map(|p| Vec2::new(p[0], p[2])).collect();
        // Each curve's rings are perpendicular to that curve: second curve, vertical in z.
        let second: Vec<_> = pts
            .iter()
            .filter(|p| p.y > 1001.0 && (p.x - 1000.0).abs() >= BUILDING_REACH - 0.01)
            .collect();
        assert!(
            !second.is_empty(),
            "rings of the second curve are across it"
        );
        // The outer corner is filled by a cap around the node.
        let q = Vec2::new(c.x + 0.7 * BUILDING_REACH, c.z - 0.7 * BUILDING_REACH);
        let tri_covers = m.indices.chunks(3).any(|tri| {
            let [p0, p1, p2] = [tri[0], tri[1], tri[2]].map(|i| pts[i as usize]);
            let s = |a: Vec2, b: Vec2, c: Vec2| (b - a).perp_dot(c - a);
            let (d0, d1, d2) = (s(p0, p1, q), s(p1, p2, q), s(p2, p0, q));
            (d0 >= 0.0 && d1 >= 0.0 && d2 >= 0.0) || (d0 <= 0.0 && d1 <= 0.0 && d2 <= 0.0)
        });
        assert!(tri_covers, "the outside of the bend has no hole");
    }

    #[test]
    fn built_pipes_and_their_band_exist_only_while_pipes_are_visible() {
        let t = slope();
        let g = line_graph();
        assert!(built_pipe_meshes(&g, &t, false, BUILDING_REACH).is_none());
        let (pipes, band) = built_pipe_meshes(&g, &t, true, BUILDING_REACH).unwrap();
        assert!(!pipes.indices.is_empty() && !band.indices.is_empty());
        let (p, b) = built_pipe_meshes(&UtilityGraph::new(), &t, true, BUILDING_REACH).unwrap();
        assert!(p.indices.is_empty() && b.indices.is_empty());
    }

    #[test]
    fn an_empty_network_draws_nothing() {
        assert!(power_mesh(&UtilityGraph::new()).indices.is_empty());
    }
}
