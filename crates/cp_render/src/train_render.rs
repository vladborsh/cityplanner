//! The intercity train on the ground track (T-167): a locomotive and six carriages, one entity
//! each, placed every frame along the polyline of the leg the train is on (`SimView::
//! intercity_train`) between two ticks (`TickFraction`), like the metro trains of T-164. The
//! cars together are as long as the `intercity` vehicle of the content (120 m).

use bevy::prelude::*;
use cp_sim::SimView;

use crate::agent_render::TickFraction;
use crate::chunk::MeshData;
use crate::material::VertexColorMaterial;
use crate::problem_render::to_bevy_mesh;
use crate::underground::cuboid;

/// Cars of the train: the locomotive and its carriages.
pub const CARS: usize = 7;
/// Length of the train when the content says nothing, metres.
const DEFAULT_LENGTH: f32 = 120.0;
/// Gap between two cars, metres.
const GAP: f32 = 0.8;
const HALF_W: f32 = 1.5;
const HALF_H: f32 = 1.75;
/// The track surface is this much above the polyline.
const RAIL_LIFT: f32 = 0.6;

const LOCO: [f32; 4] = [0.72, 0.16, 0.12, 1.0];
const COACH: [f32; 4] = [0.93, 0.90, 0.80, 1.0];
const STRIPE: [f32; 4] = [0.15, 0.30, 0.62, 1.0];
const GLASS: [f32; 4] = [0.12, 0.15, 0.2, 1.0];
const ROOF: [f32; 4] = [0.45, 0.46, 0.50, 1.0];

/// Length of one car of a train `total` metres long.
pub fn car_len(total: f32) -> f32 {
    total / CARS as f32 - GAP
}

/// A carriage `len` long on the origin, along +Z: a cream body with a blue stripe, a window
/// band and a grey roof.
pub fn coach_mesh(len: f32) -> MeshData {
    let mut m = MeshData::default();
    let c = Vec3::new(0.0, HALF_H, 0.0);
    cuboid(&mut m, c, Vec3::new(HALF_W, HALF_H, len * 0.5), COACH);
    cuboid(
        &mut m,
        c + Vec3::Y * 0.35,
        Vec3::new(HALF_W + 0.04, 0.5, len * 0.5 - 0.9),
        GLASS,
    );
    cuboid(
        &mut m,
        c - Vec3::Y * 0.85,
        Vec3::new(HALF_W + 0.05, 0.22, len * 0.5),
        STRIPE,
    );
    cuboid(
        &mut m,
        c + Vec3::Y * (HALF_H - 0.1),
        Vec3::new(HALF_W - 0.45, 0.2, len * 0.5 - 0.3),
        ROOF,
    );
    m
}

/// The locomotive `len` long on the origin, its nose towards +Z: a red body with a dark cab
/// window at the nose and a grey roof.
pub fn loco_mesh(len: f32) -> MeshData {
    let mut m = MeshData::default();
    let c = Vec3::new(0.0, HALF_H, 0.0);
    cuboid(&mut m, c, Vec3::new(HALF_W, HALF_H, len * 0.5), LOCO);
    cuboid(
        &mut m,
        c + Vec3::new(0.0, 0.65, len * 0.5 - 2.0),
        Vec3::new(HALF_W + 0.04, 0.55, 1.4),
        GLASS,
    );
    cuboid(
        &mut m,
        c - Vec3::Y * 0.85,
        Vec3::new(HALF_W + 0.05, 0.22, len * 0.5),
        STRIPE,
    );
    cuboid(
        &mut m,
        c + Vec3::Y * (HALF_H - 0.1),
        Vec3::new(HALF_W - 0.5, 0.25, len * 0.5 - 3.5),
        ROOF,
    );
    m
}

#[derive(Component)]
struct TrainCar;

#[derive(Resource, Default)]
struct TrainRender {
    cars: Vec<Entity>,
    /// The length the meshes were made for.
    length: f32,
    loco: Option<Handle<Mesh>>,
    coach: Option<Handle<Mesh>>,
    material: Option<Handle<VertexColorMaterial>>,
}

fn sync_train(
    view: Option<Res<SimView>>,
    fraction: Res<TickFraction>,
    mut st: ResMut<TrainRender>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<VertexColorMaterial>>,
    mut cars: Query<(&mut Transform, &mut Visibility), With<TrainCar>>,
) {
    let pose = view.as_deref().and_then(|v| v.intercity_train.as_deref());
    let Some(pose) = pose else {
        for e in st.cars.drain(..) {
            commands.entity(e).despawn();
        }
        return;
    };
    let total = view
        .as_deref()
        .and_then(|v| v.content.as_deref())
        .and_then(|c| c.vehicle_id("intercity").map(|id| c.vehicle(id).length))
        .unwrap_or(DEFAULT_LENGTH);
    let len = car_len(total);
    if st.length != total {
        for e in st.cars.drain(..) {
            commands.entity(e).despawn();
        }
        st.length = total;
        st.loco = Some(meshes.add(to_bevy_mesh(loco_mesh(len))));
        st.coach = Some(meshes.add(to_bevy_mesh(coach_mesh(len))));
    }
    let material = st
        .material
        .get_or_insert_with(|| materials.add(VertexColorMaterial::opaque(0.6, 0.1)))
        .clone();
    let head = pose.s + (pose.next_s - pose.s) * fraction.0.clamp(0.0, 1.0);
    for i in 0..CARS {
        // Car `i` stands `i` cars behind the head; one that has not yet come onto the line is
        // not drawn.
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
        if let Some(&e) = st.cars.get(i)
            && let Ok((mut t, mut v)) = cars.get_mut(e)
        {
            *t = tf;
            *v = visible;
        } else if st.cars.len() == i {
            let mesh = if i == 0 { &st.loco } else { &st.coach };
            let e = commands
                .spawn((
                    Mesh3d(mesh.clone().expect("meshes made")),
                    MeshMaterial3d(material.clone()),
                    tf,
                    visible,
                    TrainCar,
                    bevy::light::NotShadowCaster,
                    bevy::camera::visibility::NoFrustumCulling,
                    Name::new(if i == 0 {
                        "intercity locomotive"
                    } else {
                        "intercity carriage"
                    }),
                ))
                .id();
            st.cars.push(e);
        }
    }
}

pub struct TrainRenderPlugin;

impl Plugin for TrainRenderPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TrainRender>()
            .init_resource::<TickFraction>()
            .add_systems(Update, sync_train);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lengths(m: &MeshData) -> (f32, f32) {
        let z: Vec<f32> = m.positions.iter().map(|p| p[2]).collect();
        (
            z.iter().cloned().fold(f32::MAX, f32::min),
            z.iter().cloned().fold(f32::MIN, f32::max),
        )
    }

    #[test]
    fn the_train_is_as_long_as_the_intercity_vehicle_and_its_cars_differ() {
        let db = cp_content::ContentDb::load_dir(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets/content"
        ))
        .unwrap();
        let def = db.vehicle(db.vehicle_id("intercity").unwrap());
        let len = car_len(def.length);
        let total = CARS as f32 * len + (CARS as f32 - 1.0) * GAP;
        assert!(
            (total - def.length).abs() <= GAP,
            "{total} m of cars for a {} m train",
            def.length
        );
        let (loco, coach) = (loco_mesh(len), coach_mesh(len));
        // Each car is as long as asked, and the two are different pictures.
        for m in [&loco, &coach] {
            let (lo, hi) = lengths(m);
            assert!((hi - lo - len).abs() < 0.1, "{lo}..{hi}");
        }
        assert_ne!(loco.colors, coach.colors);
        assert!(
            loco.positions
                .iter()
                .all(|p| p.iter().all(|v| v.is_finite()))
        );
    }
}
