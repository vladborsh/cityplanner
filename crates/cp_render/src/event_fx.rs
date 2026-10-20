//! Bevy side of the city's little reactions (T-287): when the building grid of `SimView`
//! changes, [`crate::city_events`] diffs it once into [`CityChange`], and the houses' moods
//! ([`crate::mood`]) pop up over their roofs as a wave. One alpha-blended, unlit mesh entity,
//! rebuilt each frame while an emoji lives. Render time only (the app's `Time`), never the sim
//! tick; nothing here touches the sim.

use std::sync::Arc;

use bevy::camera::visibility::NoFrustumCulling;
use bevy::prelude::*;
use cp_building::BuildingGrid;
use cp_net::NetGraph;
use cp_sim::SimView;

use crate::building_render::{BuildingState, BuildingSync};
use crate::city_events::{BuildingChange, SegmentChange, building_events, segment_events};
use crate::mood::{Emoji, LIFE_S, Mood, emoji_mesh, moods_of};
use crate::problem::Facing;
use crate::problem_render::{set_effect_visible, to_bevy_mesh};

/// Emojis farther than this from the camera are not drawn, metres.
pub const EMOJI_RANGE_M: f32 = 800.0;

pub struct EventFxPlugin;

impl Plugin for EventFxPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CityChange>()
            .init_resource::<Snapshot>()
            .init_resource::<MoodFx>()
            .add_systems(
                Update,
                (diff_city, mood_demo, start_moods, draw_moods)
                    .chain()
                    .after(BuildingSync),
            );
    }
}

/// What changed in the city this frame (empty when nothing did), for the effects that react.
#[derive(Resource, Default, Debug)]
pub struct CityChange {
    pub buildings: Vec<BuildingChange>,
    pub laid: Vec<SegmentChange>,
    pub removed: Vec<SegmentChange>,
    /// The houses' reactions (T-287).
    pub moods: Vec<Mood>,
}

/// The grids the last diff was taken against.
#[derive(Resource, Default)]
struct Snapshot {
    buildings: Option<Arc<BuildingGrid>>,
    net: Option<Arc<NetGraph>>,
    tick: u64,
}

fn diff_city(view: Option<Res<SimView>>, mut snap: ResMut<Snapshot>, mut out: ResMut<CityChange>) {
    if !out.buildings.is_empty() || !out.laid.is_empty() || !out.removed.is_empty() {
        *out = CityChange::default();
    }
    let Some(view) = view else { return };
    let same_b = snap
        .buildings
        .as_ref()
        .is_some_and(|g| Arc::ptr_eq(g, &view.buildings));
    let same_n = snap.net.as_ref().is_some_and(|n| Arc::ptr_eq(n, &view.net));
    if same_b && same_n {
        return;
    }
    // The first snapshot, or the clock went back (a load): no change to react to.
    let fresh = snap.buildings.is_none() || view.tick.0 < snap.tick;
    if !fresh && let (Some(old_b), Some(old_n)) = (&snap.buildings, &snap.net) {
        let buildings = if same_b {
            Vec::new()
        } else {
            building_events(old_b, &view.buildings)
        };
        let (laid, removed) = match (&view.content, same_n) {
            (Some(db), false) => segment_events(old_n, &view.net, |r| db.road(r).width),
            _ => (Vec::new(), Vec::new()),
        };
        let moods = match &view.content {
            Some(db) if !buildings.is_empty() => {
                moods_of(&view.net, old_b, &view.buildings, db, &buildings)
            }
            _ => Vec::new(),
        };
        *out = CityChange {
            buildings,
            laid,
            removed,
            moods,
        };
    }
    snap.buildings = Some(Arc::clone(&view.buildings));
    snap.net = Some(Arc::clone(&view.net));
    snap.tick = view.tick.0;
}

/// The wave of a building already standing, held at one moment (`--mood-demo` of the desktop
/// client, T-287 shots): the houses' moods as if `def` had just opened, `age` seconds after.
#[derive(Resource, Clone, Debug)]
pub struct MoodDemo {
    pub def: String,
    pub age: f32,
}

impl MoodDemo {
    /// `elementary_school,0.3`.
    pub fn parse(spec: &str) -> Option<Self> {
        let (def, age) = spec.split_once(',')?;
        Some(Self {
            def: def.to_string(),
            age: age.parse().ok()?,
        })
    }
}

fn mood_demo(
    demo: Option<Res<MoodDemo>>,
    view: Option<Res<SimView>>,
    buildings: Res<BuildingState>,
    mut fx: ResMut<MoodFx>,
) {
    let (Some(demo), Some(view), Some(db)) = (
        demo,
        view.as_deref(),
        view.as_deref().and_then(|v| v.content.as_deref()),
    ) else {
        return;
    };
    if fx.frozen.is_some() {
        return;
    }
    let Some(def) = db.building_id(&demo.def) else {
        return;
    };
    let Some((id, _)) = view.buildings.iter().find(|(_, b)| b.def == def) else {
        return;
    };
    // Once the roofs are built, so the emojis stand on them.
    if buildings.set.piece(id).is_none() {
        return;
    }
    let mut old = (*view.buildings).clone();
    old.remove(id);
    for m in crate::mood::moods(&view.net, &old, &view.buildings, db) {
        let roof = buildings.set.piece(m.building).map_or_else(
            || Vec3::new(m.pos.x, 6.0, m.pos.y),
            |p| Vec3::new(m.pos.x, p.max.y, m.pos.y),
        );
        fx.live.push(Live {
            mood: m,
            at: m.delay,
            roof,
        });
    }
    fx.frozen = Some(demo.age);
}

/// A mood waiting for its turn in the wave, or showing.
#[derive(Clone, Copy, Debug)]
struct Live {
    mood: Mood,
    /// Render time it shows, seconds.
    at: f32,
    /// Its roof, world.
    roof: Vec3,
}

#[derive(Resource, Default)]
struct MoodFx {
    live: Vec<Live>,
    /// [`MoodDemo`]: render time stands at this many seconds after the wave started.
    frozen: Option<f32>,
    mesh: Option<Handle<Mesh>>,
    entity: Option<Entity>,
}

fn start_moods(
    time: Res<Time>,
    view: Option<Res<SimView>>,
    change: Res<CityChange>,
    buildings: Res<BuildingState>,
    mut fx: ResMut<MoodFx>,
) {
    let Some(view) = view else { return };
    if fx.frozen.is_some() {
        return;
    }
    let now = time.elapsed_secs();
    fx.live.retain(|l| now - l.at < LIFE_S);
    for m in &change.moods {
        let roof = match buildings.set.piece(m.building) {
            Some(p) => Vec3::new(m.pos.x, p.max.y, m.pos.y),
            None => {
                let y = view.terrain.as_deref().map_or(0.0, |t| t.height(m.pos));
                Vec3::new(m.pos.x, y + 6.0, m.pos.y)
            }
        };
        // A newer mood of the same house replaces the old one.
        fx.live.retain(|l| l.mood.building != m.building);
        fx.live.push(Live {
            mood: *m,
            at: now + m.delay,
            roof,
        });
    }
}

fn draw_moods(
    time: Res<Time>,
    camera: Query<&GlobalTransform, With<Camera3d>>,
    mut fx: ResMut<MoodFx>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut commands: Commands,
) {
    let Some(cam) = camera.iter().next() else {
        return;
    };
    let (_, rotation, position) = cam.to_scale_rotation_translation();
    let facing = Facing {
        position,
        right: rotation * Vec3::X,
        up: rotation * Vec3::Y,
    };
    let now = fx.frozen.unwrap_or_else(|| time.elapsed_secs());
    let emojis: Vec<Emoji> = fx
        .live
        .iter()
        .filter(|l| l.roof.distance(position) <= EMOJI_RANGE_M)
        .map(|l| Emoji {
            at: l.roof,
            happy: l.mood.happy,
            age: now - l.at,
        })
        .collect();
    let data = emoji_mesh(&emojis, &facing);
    if !set_effect_visible(!data.indices.is_empty(), fx.entity, &mut commands) {
        return;
    }
    let mesh = to_bevy_mesh(data);
    match fx.mesh.clone() {
        Some(h) => {
            let _ = meshes.insert(&h, mesh);
        }
        None => {
            let h = meshes.add(mesh);
            // The fire's material (pre-warmed at start-up, `crate::warmup`).
            let material = materials.add(StandardMaterial {
                base_color: Color::WHITE,
                unlit: true,
                cull_mode: None,
                alpha_mode: AlphaMode::Blend,
                ..default()
            });
            fx.entity = Some(
                commands
                    .spawn((
                        Mesh3d(h.clone()),
                        MeshMaterial3d(material),
                        Transform::default(),
                        NoFrustumCulling,
                        Name::new("moods"),
                    ))
                    .id(),
            );
            fx.mesh = Some(h);
        }
    }
}
