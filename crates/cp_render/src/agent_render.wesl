// Instanced vehicles and pedestrians. One draw per (kind, detail): the mesh is the model,
// the per-instance vertex buffer places it. WebGL2-safe (plain instance-stepped attributes).
//
// Pedestrians sway in this vertex shader: the head side-steps and the body bobs with the
// per-instance gait phase, scaled by the instance's sway amplitude (0 = standing).
//
// Shadows (BUG-063): agents receive the sun's cascaded shadows, and cast their own as a
// second draw of the same instances (`vertex_shadow`/`fragment_shadow`): the model flattened
// along the sun direction onto a plane just above the ground it stands on, multiplied onto
// what is drawn there. That pass writes depth and tests strictly, so overlapping triangles
// of the flattened model darken a pixel once.
import bevy_pbr::render::mesh_view_bindings::{view, lights};
import bevy_pbr::render::mesh_view_types::DIRECTIONAL_LIGHT_FLAGS_SHADOWS_ENABLED_BIT;
import bevy_pbr::render::shadows::fetch_directional_shadow;

struct Vertex {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(5) color: vec4<f32>,
    @location(8) pos_yaw: vec4<f32>,
    @location(9) scale_phase: vec4<f32>,
    @location(10) color_sway: vec4<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) color: vec3<f32>,
    @location(2) world_position: vec3<f32>,
};

struct ShadowOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) world_position: vec3<f32>,
};

// Height of the cast shadow's plane above the instance origin, metres: the road surface for
// vehicles (origin on the lane curve, road 0.06 above), the sidewalk for pedestrians (origin
// on the terrain, sidewalk 0.21 above). Only pedestrians carry a gait phase.
const VEHICLE_SHADOW_LIFT: f32 = 0.06;
const PEDESTRIAN_SHADOW_LIFT: f32 = 0.21;
// The shadow is drawn this far towards the camera (a depth bias in metres): lane curves and
// the terrain under a sidewalk differ by a few decimetres, and the shadow must still win over
// the ground it lies on, while a car or a wall in front of it keeps hiding it.
const SHADOW_PULL: f32 = 0.5;
// See `fragment_shadow`.
const AMBIENT_GAIN: f32 = 12.0;
// Below this sun height (sine of the elevation) the shadow would stretch too far: none.
const MIN_SUN_Y: f32 = 0.08;

// Unit vector from the ground towards the sun (xyz).
@group(3) @binding(0) var<uniform> to_sun_uniform: vec4<f32>;

fn world_of(v: Vertex) -> vec3<f32> {
    var p = v.position * v.scale_phase.xyz;
    // Gait: sideways sway grows with height, plus a small bob.
    let sway = v.color_sway.w;
    let phase = v.scale_phase.w;
    p.x += sin(phase) * sway * (v.position.y / 1.8);
    p.y += abs(sin(phase)) * sway * 0.5;

    let c = cos(v.pos_yaw.w);
    let s = sin(v.pos_yaw.w);
    // Yaw about Y with forward = +z: local (x, z) -> world (x c + z s, -x s + z c).
    return vec3<f32>(p.x * c + p.z * s, p.y, -p.x * s + p.z * c) + v.pos_yaw.xyz;
}

fn view_z_of(world: vec3<f32>) -> f32 {
    return dot(vec4<f32>(
        view.view_from_world[0].z,
        view.view_from_world[1].z,
        view.view_from_world[2].z,
        view.view_from_world[3].z,
    ), vec4<f32>(world, 1.0));
}

// How much of the sun reaches `world` (1 = lit, 0 = in a shadow of the scene).
fn sun_visibility(world: vec3<f32>, normal: vec3<f32>, frag_coord: vec2<f32>) -> f32 {
    if ((lights.directional_lights[0u].flags & DIRECTIONAL_LIGHT_FLAGS_SHADOWS_ENABLED_BIT) == 0u) {
        return 1.0;
    }
    return fetch_directional_shadow(0u, vec4<f32>(world, 1.0), normal, view_z_of(world), frag_coord);
}

@vertex
fn vertex(v: Vertex) -> VertexOutput {
    let world = world_of(v);
    let c = cos(v.pos_yaw.w);
    let s = sin(v.pos_yaw.w);
    let n = vec3<f32>(v.normal.x * c + v.normal.z * s, v.normal.y, -v.normal.x * s + v.normal.z * c);

    var out: VertexOutput;
    out.clip_position = view.clip_from_world * vec4<f32>(world, 1.0);
    out.normal = n;
    out.color = v.color.rgb * mix(vec3<f32>(1.0), v.color_sway.rgb, v.color.a);
    out.world_position = world;
    return out;
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let n = normalize(in.normal);
    let light = lights.directional_lights[0u];
    var diffuse = max(dot(n, light.direction_to_light), 0.0);
    if (diffuse > 0.0) {
        diffuse *= sun_visibility(in.world_position, n, in.clip_position.xy);
    }
    // Same units as the PBR path: illuminance and ambient scaled by the view exposure.
    let radiance = (light.color.rgb * diffuse / 3.14159 + lights.ambient_color.rgb) * view.exposure;
    return vec4<f32>(in.color * radiance, 1.0);
}

@vertex
fn vertex_shadow(v: Vertex) -> ShadowOutput {
    let to_sun = to_sun_uniform.xyz;
    var out: ShadowOutput;
    if (to_sun.y < MIN_SUN_Y) {
        // Degenerate: every vertex at the same point outside the clip volume.
        out.clip_position = vec4<f32>(0.0, 0.0, -2.0, 1.0);
        out.world_position = vec3<f32>(0.0);
        return out;
    }
    let pedestrian = v.scale_phase.w != 0.0 || v.color_sway.w != 0.0;
    let plane = v.pos_yaw.y + select(VEHICLE_SHADOW_LIFT, PEDESTRIAN_SHADOW_LIFT, pedestrian);
    let world = world_of(v);
    // Slide along the sun ray down (or, for points under it, up) to the plane.
    let ground = world - to_sun * ((world.y - plane) / to_sun.y);
    let pulled = ground + normalize(view.world_position - ground) * SHADOW_PULL;
    out.clip_position = view.clip_from_world * vec4<f32>(pulled, 1.0);
    out.world_position = ground;
    return out;
}

@fragment
fn fragment_shadow(in: ShadowOutput) -> @location(0) vec4<f32> {
    let light = lights.directional_lights[0u];
    // Ground lit by sun + ambient, against ambient only: the factor a shadow of the scene
    // leaves, so agent shadows match the buildings' and follow the light through the day
    // (weak, long ones at dusk). The PBR ground takes more from the ambient than a plain
    // Lambert term; AMBIENT_GAIN is calibrated on screenshots so that both shadows leave the
    // same share of light (about 45 % at the always-day sun). Where the ground is already in
    // a scene shadow, the factor fades to 1 (no double darkening).
    let sun = light.color.rgb * max(light.direction_to_light.y, 0.0) / 3.14159;
    let ambient = lights.ambient_color.rgb * AMBIENT_GAIN;
    let shadowed = ambient / max(ambient + sun, vec3<f32>(1e-5));
    let lit = sun_visibility(in.world_position, vec3<f32>(0.0, 1.0, 0.0), in.clip_position.xy);
    return vec4<f32>(mix(vec3<f32>(1.0), shadowed, lit), 1.0);
}
