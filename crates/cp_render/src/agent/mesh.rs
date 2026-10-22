//! Procedural low-poly meshes of vehicles and pedestrians (self-made, no imported assets).
//!
//! Vehicle meshes are unit-sized: x = width, y = height, z = length (front at +z), origin at
//! the ground in the middle; the instance scale makes them the `VehicleDef` size. Pedestrians
//! are in metres (1.8 m tall) and only scaled in height. Vertex alpha 1 = tinted by the
//! instance colour, alpha 0 = fixed colour (glass, wheels, skin).

use crate::chunk::MeshData;

const GLASS: [f32; 4] = [0.05, 0.07, 0.10, 0.0];
const TYRE: [f32; 4] = [0.02, 0.02, 0.02, 0.0];
const SKIN: [f32; 4] = [0.60, 0.42, 0.32, 0.0];
const TINT: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
const BUS_CREAM: [f32; 4] = [0.93, 0.90, 0.80, 0.0];
const BUS_ROOF: [f32; 4] = [0.55, 0.58, 0.62, 0.0];
/// The pantograph of a tram (T-302b): dark steel, no instance colour.
const PANTOGRAPH: [f32; 4] = [0.22, 0.23, 0.25, 0.0];
const MAIL_RED: [f32; 4] = [0.78, 0.10, 0.08, 0.0];
const MAIL_ROOF: [f32; 4] = [0.93, 0.93, 0.90, 0.0];

pub(crate) fn push_box(m: &mut MeshData, min: [f32; 3], max: [f32; 3], color: [f32; 4]) {
    let [x0, y0, z0] = min;
    let [x1, y1, z1] = max;
    // (normal, four corners counter-clockwise seen from outside)
    let faces: [([f32; 3], [[f32; 3]; 4]); 6] = [
        (
            [0.0, 1.0, 0.0],
            [[x0, y1, z1], [x1, y1, z1], [x1, y1, z0], [x0, y1, z0]],
        ),
        (
            [0.0, -1.0, 0.0],
            [[x0, y0, z0], [x1, y0, z0], [x1, y0, z1], [x0, y0, z1]],
        ),
        (
            [0.0, 0.0, 1.0],
            [[x0, y0, z1], [x1, y0, z1], [x1, y1, z1], [x0, y1, z1]],
        ),
        (
            [0.0, 0.0, -1.0],
            [[x1, y0, z0], [x0, y0, z0], [x0, y1, z0], [x1, y1, z0]],
        ),
        (
            [1.0, 0.0, 0.0],
            [[x1, y0, z1], [x1, y0, z0], [x1, y1, z0], [x1, y1, z1]],
        ),
        (
            [-1.0, 0.0, 0.0],
            [[x0, y0, z0], [x0, y0, z1], [x0, y1, z1], [x0, y1, z0]],
        ),
    ];
    for (n, corners) in faces {
        let base = m.positions.len() as u32;
        for c in corners {
            m.positions.push(c);
            m.normals.push(n);
            m.colors.push(color);
        }
        m.indices
            .extend([base, base + 1, base + 2, base, base + 2, base + 3]);
    }
}

/// A cylinder along z from `z0` to `z1` around `(cx, cy)` with the radii `rx`, `ry` (unit
/// coordinates: the instance scale turns a round tube into an ellipse, so the radii are given
/// per axis) and end caps of colour `cap`.
fn push_cylinder(
    m: &mut MeshData,
    (cx, cy): (f32, f32),
    (rx, ry): (f32, f32),
    (z0, z1): (f32, f32),
    color: [f32; 4],
    cap: [f32; 4],
) {
    const SIDES: usize = 12;
    let at = |k: usize| {
        let a = k as f32 / SIDES as f32 * std::f32::consts::TAU;
        (a.cos(), a.sin())
    };
    for k in 0..SIDES {
        let ((c0, s0), (c1, s1)) = (at(k), at(k + 1));
        let mid = (k as f32 + 0.5) / SIDES as f32 * std::f32::consts::TAU;
        let n = [mid.cos(), mid.sin(), 0.0];
        let base = m.positions.len() as u32;
        for (c, s, z) in [(c0, s0, z0), (c1, s1, z0), (c1, s1, z1), (c0, s0, z1)] {
            m.positions.push([cx + rx * c, cy + ry * s, z]);
            m.normals.push(n);
            m.colors.push(color);
        }
        m.indices
            .extend([base, base + 1, base + 2, base, base + 2, base + 3]);
        for (z, nz) in [(z1, 1.0f32), (z0, -1.0)] {
            let base = m.positions.len() as u32;
            m.positions.push([cx, cy, z]);
            m.positions.push([cx + rx * c0, cy + ry * s0, z]);
            m.positions.push([cx + rx * c1, cy + ry * s1, z]);
            for _ in 0..3 {
                m.normals.push([0.0, 0.0, nz]);
                m.colors.push(cap);
            }
            if nz > 0.0 {
                m.indices.extend([base, base + 1, base + 2]);
            } else {
                m.indices.extend([base, base + 2, base + 1]);
            }
        }
    }
}

fn wheels(m: &mut MeshData, z: &[f32]) {
    for &zc in z {
        for xs in [-1.0f32, 1.0] {
            let x = xs * 0.47;
            push_box(
                m,
                [x - 0.07, 0.0, zc - 0.09],
                [x + 0.07, 0.24, zc + 0.09],
                TYRE,
            );
        }
    }
}

/// The chain trucks (T-213b), kinds 7..=10: a cab in front (instance colour), a glass front, a
/// dark chassis and a body of its own: a flatbed with crates and bales, a log trailer, an ore
/// tipper, a tanker.
fn chain_truck(m: &mut MeshData, kind: usize) {
    push_box(m, [-0.5, 0.12, 0.28], [0.5, 0.78, 0.5], TINT);
    push_box(m, [-0.46, 0.5, 0.46], [0.46, 0.74, 0.505], GLASS);
    let chassis = [0.2, 0.2, 0.22, 0.0];
    push_box(m, [-0.45, 0.12, -0.5], [0.45, 0.2, 0.26], chassis);
    match kind {
        7 => {
            // Flatbed with two crates and a hay bale.
            push_box(
                m,
                [-0.52, 0.2, -0.5],
                [0.52, 0.28, 0.22],
                [0.5, 0.36, 0.2, 0.0],
            );
            push_box(
                m,
                [-0.46, 0.28, 0.0],
                [-0.02, 0.72, 0.2],
                [0.72, 0.52, 0.25, 0.0],
            );
            push_box(
                m,
                [0.02, 0.28, 0.0],
                [0.46, 0.72, 0.2],
                [0.72, 0.52, 0.25, 0.0],
            );
            push_box(
                m,
                [-0.46, 0.28, -0.46],
                [0.46, 0.8, -0.04],
                [0.85, 0.75, 0.3, 0.0],
            );
        }
        8 => {
            // Log trailer: an open flatbed with tall stakes and five round brown logs lying
            // along it (three below, two on top), their cut ends lighter.
            let bark = [0.38, 0.22, 0.10, 0.0];
            let cut = [0.72, 0.55, 0.32, 0.0];
            push_box(m, [-0.52, 0.2, -0.5], [0.52, 0.26, 0.22], chassis);
            for x in [-0.5f32, 0.5] {
                for z in [-0.48f32, -0.2, 0.08, 0.2] {
                    push_box(
                        m,
                        [x - 0.03, 0.26, z - 0.02],
                        [x + 0.03, 1.0, z + 0.02],
                        chassis,
                    );
                }
            }
            // 1 m logs on a 2.5 x 3.4 m unit box: radius 0.5 m.
            let r = (0.5 / 2.5, 0.5 / 3.4);
            for (x, y, z0) in [
                (-0.3f32, 0.26 + r.1, -0.56f32),
                (0.0, 0.26 + r.1, -0.5),
                (0.3, 0.26 + r.1, -0.56),
                (-0.15, 0.26 + 3.0 * r.1, -0.5),
                (0.15, 0.26 + 3.0 * r.1, -0.56),
            ] {
                push_cylinder(m, (x, y), r, (z0, z0 + 0.8), bark, cut);
            }
        }
        9 => {
            // Ore tipper: a dark open skip, tilted up at the back by a stepped floor.
            push_box(
                m,
                [-0.52, 0.2, -0.5],
                [0.52, 0.34, 0.22],
                [0.3, 0.3, 0.32, 0.0],
            );
            push_box(
                m,
                [-0.52, 0.34, -0.5],
                [0.52, 0.86, -0.4],
                [0.3, 0.3, 0.32, 0.0],
            );
            push_box(
                m,
                [-0.52, 0.34, -0.5],
                [-0.42, 0.78, 0.22],
                [0.3, 0.3, 0.32, 0.0],
            );
            push_box(
                m,
                [0.42, 0.34, -0.5],
                [0.52, 0.78, 0.22],
                [0.3, 0.3, 0.32, 0.0],
            );
            push_box(
                m,
                [-0.42, 0.34, -0.4],
                [0.42, 0.7, 0.2],
                [0.55, 0.38, 0.28, 0.0],
            );
        }
        _ => {
            // Tanker: a silver horizontal round tank on the trailer behind the cab, with a
            // hatch on top and an orange hazard band.
            let tank = [0.80, 0.82, 0.86, 0.0];
            let r = (1.0 / 2.5, 1.0 / 3.4);
            push_box(m, [-0.4, 0.2, -0.5], [0.4, 0.3, 0.22], chassis);
            push_cylinder(m, (0.0, 0.3 + r.1), r, (-0.5, 0.2), tank, tank);
            push_cylinder(
                m,
                (0.0, 0.3 + r.1),
                (r.0 * 1.03, r.1 * 1.03),
                (-0.12, -0.04),
                [0.85, 0.5, 0.1, 0.0],
                [0.85, 0.5, 0.1, 0.0],
            );
            push_box(
                m,
                [-0.1, 0.3 + 2.0 * r.1, -0.3],
                [0.1, 0.3 + 2.0 * r.1 + 0.05, -0.1],
                [0.3, 0.3, 0.32, 0.0],
            );
        }
    }
    wheels(m, &[0.4, -0.2, -0.4]);
}

/// Mesh of a vehicle kind (`VehicleKind` as a number) at a detail level (0 full, 1 one box).
pub fn vehicle_mesh(kind: usize, detail: usize) -> MeshData {
    let mut m = MeshData::default();
    if detail > 0 {
        push_box(&mut m, [-0.5, 0.1, -0.5], [0.5, 0.9, 0.5], TINT);
        return m;
    }
    match kind {
        1 => {
            // Truck: cab in front, cargo box behind.
            push_box(&mut m, [-0.5, 0.12, 0.2], [0.5, 0.8, 0.5], TINT);
            push_box(&mut m, [-0.46, 0.5, 0.42], [0.46, 0.75, 0.505], GLASS);
            push_box(
                &mut m,
                [-0.5, 0.18, -0.5],
                [0.5, 1.0, 0.15],
                [0.8, 0.8, 0.78, 0.0],
            );
            wheels(&mut m, &[0.35, -0.3, -0.42]);
        }
        7..=10 => chain_truck(&mut m, kind),
        13 => {
            // Mail van (T-248): a high-roofed van in the post yellow (the instance colour), the
            // bonnet lower in front, a red band along the sides and a white roof.
            push_box(&mut m, [-0.5, 0.12, -0.5], [0.5, 0.96, 0.3], TINT);
            push_box(&mut m, [-0.5, 0.12, 0.3], [0.5, 0.52, 0.5], TINT);
            push_box(&mut m, [-0.505, 0.42, -0.5], [0.505, 0.54, 0.3], MAIL_RED);
            push_box(&mut m, [-0.46, 0.54, 0.29], [0.46, 0.9, 0.31], GLASS);
            push_box(&mut m, [-0.505, 0.6, 0.12], [0.505, 0.86, 0.26], GLASS);
            push_box(&mut m, [-0.47, 0.96, -0.47], [0.47, 1.0, 0.27], MAIL_ROOF);
            wheels(&mut m, &[0.3, -0.32]);
        }
        6 => {
            // Intercity train (T-167): a long body with a window band; the client draws it car
            // by car along the track (`train_render`), this is the one-piece stand-in.
            push_box(&mut m, [-0.5, 0.1, -0.5], [0.5, 0.9, 0.5], TINT);
            push_box(&mut m, [-0.52, 0.5, -0.45], [0.52, 0.75, 0.45], GLASS);
        }
        5 => {
            // Goods truck (T-169b): a short cab with a glass front, a long box trailer on a
            // lower chassis, a neutral light grey that takes no instance colour.
            push_box(&mut m, [-0.5, 0.12, 0.28], [0.5, 0.78, 0.5], TINT);
            push_box(&mut m, [-0.46, 0.5, 0.46], [0.46, 0.74, 0.505], GLASS);
            push_box(
                &mut m,
                [-0.45, 0.12, -0.5],
                [0.45, 0.2, 0.26],
                [0.25, 0.25, 0.27, 0.0],
            );
            push_box(
                &mut m,
                [-0.52, 0.2, -0.5],
                [0.52, 1.0, 0.22],
                [0.78, 0.79, 0.76, 0.0],
            );
            wheels(&mut m, &[0.4, -0.2, -0.4]);
        }
        2 => {
            // Bus (T-158): a blue lower body, a cream upper one with a window band along the
            // sides and glass at both ends, a grey roof. The lower body takes the instance colour: the colour of the bus's line.
            push_box(&mut m, [-0.5, 0.1, -0.5], [0.5, 0.52, 0.5], TINT);
            push_box(&mut m, [-0.5, 0.52, -0.5], [0.5, 0.98, 0.5], BUS_CREAM);
            push_box(&mut m, [-0.505, 0.6, -0.46], [0.505, 0.88, 0.46], GLASS);
            push_box(&mut m, [-0.46, 0.6, 0.495], [0.46, 0.9, 0.505], GLASS);
            push_box(&mut m, [-0.46, 0.6, -0.505], [0.46, 0.9, -0.495], GLASS);
            push_box(&mut m, [-0.45, 0.98, -0.45], [0.45, 1.0, 0.45], BUS_ROOF);
            wheels(&mut m, &[0.32, -0.32]);
        }
        14 => {
            // Tram (T-302b): a long low-floor body in three sections, the lower half in the
            // line's colour (the instance colour), a cream upper half with one long window band,
            // dark joints between the sections, glass at both ends and a pantograph on the roof.
            push_box(&mut m, [-0.5, 0.06, -0.5], [0.5, 0.42, 0.5], TINT);
            push_box(&mut m, [-0.5, 0.42, -0.5], [0.5, 0.9, 0.5], BUS_CREAM);
            push_box(&mut m, [-0.505, 0.48, -0.47], [0.505, 0.82, 0.47], GLASS);
            push_box(&mut m, [-0.46, 0.45, 0.495], [0.46, 0.84, 0.505], GLASS);
            push_box(&mut m, [-0.46, 0.45, -0.505], [0.46, 0.84, -0.495], GLASS);
            for z in [-0.17, 0.17] {
                push_box(
                    &mut m,
                    [-0.51, 0.06, z - 0.012],
                    [0.51, 0.9, z + 0.012],
                    [0.18, 0.19, 0.21, 0.0],
                );
            }
            push_box(&mut m, [-0.44, 0.9, -0.48], [0.44, 0.93, 0.48], BUS_ROOF);
            // Pantograph: a base, a raised arm and the bar on the wire.
            push_box(&mut m, [-0.3, 0.93, -0.04], [0.3, 0.96, 0.04], PANTOGRAPH);
            push_box(&mut m, [-0.04, 0.96, -0.02], [0.04, 1.14, 0.02], PANTOGRAPH);
            push_box(
                &mut m,
                [-0.35, 1.14, -0.015],
                [0.35, 1.16, 0.015],
                PANTOGRAPH,
            );
            wheels(&mut m, &[0.4, 0.0, -0.4]);
        }
        _ => {
            // Car (and emergency vehicle): body, glass cabin, roof, wheels.
            push_box(&mut m, [-0.5, 0.15, -0.5], [0.5, 0.55, 0.5], TINT);
            push_box(&mut m, [-0.44, 0.55, -0.28], [0.44, 0.92, 0.18], GLASS);
            push_box(&mut m, [-0.46, 0.92, -0.3], [0.46, 1.0, 0.2], TINT);
            wheels(&mut m, &[0.33, -0.33]);
            if kind == 3 {
                push_box(
                    &mut m,
                    [-0.3, 1.0, -0.1],
                    [0.3, 1.08, 0.05],
                    [0.9, 0.1, 0.1, 0.0],
                );
            }
        }
    }
    m
}

/// Mesh of a pedestrian at a detail level (0 legs/torso/head, 1 one box).
pub fn pedestrian_mesh(detail: usize) -> MeshData {
    let mut m = MeshData::default();
    if detail > 0 {
        push_box(&mut m, [-0.2, 0.0, -0.12], [0.2, 1.7, 0.12], TINT);
        return m;
    }
    push_box(
        &mut m,
        [-0.16, 0.0, -0.09],
        [0.16, 0.8, 0.09],
        [0.10, 0.12, 0.2, 0.0],
    );
    push_box(&mut m, [-0.22, 0.8, -0.12], [0.22, 1.45, 0.12], TINT);
    push_box(&mut m, [-0.1, 1.45, -0.1], [0.1, 1.75, 0.1], SKIN);
    m
}

/// A bus shelter in metres (T-158): x along the road, z away from it (the open side faces the
/// road, z < 0), origin on the pavement at the stop. Colours are fixed (alpha 0).
pub fn shelter_mesh() -> MeshData {
    const STEEL: [f32; 4] = [0.30, 0.33, 0.37, 0.0];
    const ROOF: [f32; 4] = [0.15, 0.40, 0.78, 0.0];
    const PANEL: [f32; 4] = [0.55, 0.72, 0.80, 0.0];
    const BENCH: [f32; 4] = [0.45, 0.30, 0.18, 0.0];
    let mut m = MeshData::default();
    // Roof slab, back panel, two posts, a bench, and the stop pole with its sign.
    push_box(&mut m, [-1.6, 2.4, -0.7], [1.6, 2.55, 0.9], ROOF);
    push_box(&mut m, [-1.55, 0.2, 0.82], [1.55, 2.4, 0.9], PANEL);
    push_box(&mut m, [-1.6, 0.0, -0.65], [-1.5, 2.4, -0.55], STEEL);
    push_box(&mut m, [1.5, 0.0, -0.65], [1.6, 2.4, -0.55], STEEL);
    push_box(&mut m, [-1.0, 0.45, 0.4], [1.0, 0.55, 0.78], BENCH);
    push_box(&mut m, [2.2, 0.0, -0.9], [2.28, 3.0, -0.82], STEEL);
    push_box(&mut m, [1.95, 2.7, -0.95], [2.55, 3.3, -0.85], ROOF);
    m
}
