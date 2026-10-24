//! Bevy side of the thought bubbles ([`crate::thought`], T-318d): once a game hour the hour's
//! bubbles start over the unhappy homes in view, one after another; the selected citizen's
//! bubble stays over them while they are selected. One alpha-blended, unlit mesh entity rebuilt
//! each frame while a bubble lives, like the emojis ([`crate::event_fx`]); a home whose emoji
//! waits or shows gets no bubble. Render time only, nothing here touches the sim.

use bevy::camera::visibility::NoFrustumCulling;
use bevy::light::NotShadowCaster;
use bevy::prelude::*;
use cp_building::{BuildingId, Complaint};
use cp_core::time::TICKS_PER_DAY;
use cp_sim::SimView;

use crate::building_render::{BuildingState, BuildingSync};
use crate::event_fx::{EMOJI_RANGE_M, MoodFx};
use crate::problem::Facing;
use crate::problem_render::{set_effect_visible, to_bevy_mesh};
use crate::thought::{
    Bubble, Glyph, LIFE_S, WAVE_S, centre, glyph_of, pick_thoughts, radius, thought_mesh,
};

pub struct ThoughtPlugin;

impl Plugin for ThoughtPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ThoughtFx>()
            .init_resource::<SelectedThought>()
            .init_resource::<ShownThoughts>()
            .add_systems(
                Update,
                (start_thoughts, draw_thoughts)
                    .chain()
                    .after(BuildingSync)
                    .after(crate::event_fx::EventFxSet),
            );
    }
}

/// What the selected citizen thinks and where their agent is, world (filled by the tools).
#[derive(Resource, Default, Debug, Clone, Copy, PartialEq)]
pub struct SelectedThought(pub Option<(Vec3, Complaint)>);

/// The bubbles drawn this frame: their home, centre and half width, world (a click on one
/// selects the home).
#[derive(Resource, Default, Debug, Clone)]
pub struct ShownThoughts(pub Vec<(BuildingId, Vec3, f32)>);

/// `--thought-demo AGE` of the desktop client (T-318d shots): the hour's bubbles start at once
/// when the first roofs stand, over any home with a complaint (not only the unhappy ones), and
/// render time stands `age` seconds after.
#[derive(Resource, Clone, Copy, Debug)]
pub struct ThoughtDemo {
    pub age: f32,
}

/// A bubble waiting for its turn, or showing.
#[derive(Clone, Copy, Debug)]
struct Live {
    building: BuildingId,
    glyph: Glyph,
    /// Render time it shows, seconds.
    at: f32,
    roof: Vec3,
}

#[derive(Resource, Default)]
struct ThoughtFx {
    live: Vec<Live>,
    /// The game hour of the last pick.
    hour: Option<u64>,
    frozen: Option<f32>,
    mesh: Option<Handle<Mesh>>,
    entity: Option<Entity>,
}

/// The glyph of the selected citizen's thought: the home's glyph, a cross for illness (no
/// problem icon hangs over a walker).
fn citizen_glyph(c: Complaint) -> Option<Glyph> {
    glyph_of(c).or((c == Complaint::Sick).then_some(Glyph::Cross))
}

#[allow(clippy::too_many_arguments)]
fn start_thoughts(
    time: Res<Time>,
    view: Option<Res<SimView>>,
    camera: Query<&GlobalTransform, With<Camera3d>>,
    buildings: Res<BuildingState>,
    moods: Res<MoodFx>,
    demo: Option<Res<ThoughtDemo>>,
    mut fx: ResMut<ThoughtFx>,
) {
    let (Some(view), Some(cam)) = (view, camera.iter().next()) else {
        return;
    };
    let Some(db) = view.content.as_deref() else {
        return;
    };
    if fx.frozen.is_some() {
        return;
    }
    let hour = view.tick.0 * 24 / TICKS_PER_DAY;
    if demo.is_none() && fx.hour == Some(hour) {
        return;
    }
    let (_, rotation, position) = cam.to_scale_rotation_translation();
    let forward = rotation * Vec3::NEG_Z;
    let roof = |id: BuildingId| -> Option<Vec3> {
        let b = view.buildings.get(id)?;
        let top = buildings.set.piece(id)?.max.y;
        Some(Vec3::new(b.pos.x, top, b.pos.y))
    };
    let now = time.elapsed_secs();
    let in_view = |id: BuildingId| {
        roof(id).is_some_and(|r| {
            let to = r - position;
            to.length() <= EMOJI_RANGE_M && to.normalize_or_zero().dot(forward) > 0.5
        }) && !moods.has(id, now)
    };
    // The demo shows every home with a complaint, whatever its index (a young town stands
    // about the line).
    let below = if demo.is_some() {
        u8::MAX
    } else {
        db.balance().contentment.complain_below
    };
    let picked = pick_thoughts(&view.home_contentment, below, hour, in_view);
    if demo.is_some() && picked.is_empty() {
        // The demo waits for the roofs and the first contentment pass.
        return;
    }
    fx.hour = Some(hour);
    fx.live.retain(|l| now - l.at < LIFE_S);
    let start = if demo.is_some() { 0.0 } else { now };
    let n = picked.len().max(1) as f32;
    for (i, (id, c)) in picked.into_iter().enumerate() {
        let (Some(glyph), Some(roof)) = (glyph_of(c), roof(id)) else {
            continue;
        };
        fx.live.retain(|l| l.building != id);
        fx.live.push(Live {
            building: id,
            glyph,
            at: start + WAVE_S * i as f32 / n,
            roof,
        });
    }
    if let Some(d) = demo {
        fx.frozen = Some(d.age);
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_thoughts(
    time: Res<Time>,
    camera: Query<&GlobalTransform, With<Camera3d>>,
    selected: Res<SelectedThought>,
    moods: Res<MoodFx>,
    mut fx: ResMut<ThoughtFx>,
    mut shown: ResMut<ShownThoughts>,
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
    let mut bubbles = Vec::new();
    shown.0.clear();
    for l in &fx.live {
        // An emoji that came up meanwhile takes the roof.
        if l.roof.distance(position) > EMOJI_RANGE_M || moods.has(l.building, now) {
            continue;
        }
        let b = Bubble {
            at: l.roof,
            glyph: l.glyph,
            age: now - l.at,
            held: false,
        };
        if (0.0..LIFE_S).contains(&b.age) {
            let c = centre(&b);
            shown.0.push((l.building, c, radius(c.distance(position))));
        }
        bubbles.push(b);
    }
    if let Some((at, c)) = selected.0
        && let Some(glyph) = citizen_glyph(c)
    {
        // Lower over a walker or a car than over a roof.
        bubbles.push(Bubble {
            at: at - Vec3::Y * 3.0,
            glyph,
            age: 1.0,
            held: true,
        });
    }
    let data = thought_mesh(&bubbles, &facing);
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
                        NotShadowCaster,
                        Name::new("thoughts"),
                    ))
                    .id(),
            );
            fx.mesh = Some(h);
        }
    }
}
