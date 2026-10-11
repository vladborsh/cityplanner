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

fn push_box(m: &mut MeshData, min: [f32; 3], max: [f32; 3], color: [f32; 4]) {
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
        2 => {
            // Bus: one long body with a window band.
            push_box(&mut m, [-0.5, 0.1, -0.5], [0.5, 1.0, 0.5], TINT);
            push_box(&mut m, [-0.505, 0.5, -0.46], [0.505, 0.85, 0.46], GLASS);
            wheels(&mut m, &[0.32, -0.32]);
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
