// Instanced vehicles and pedestrians. One draw per (kind, detail): the mesh is the model,
// the per-instance vertex buffer places it. WebGL2-safe (plain instance-stepped attributes).
//
// Pedestrians walk in this vertex shader (T-297): legs and arms swing about the hip and the
// shoulder, the head side-steps and the body bobs, all with the per-instance gait phase (the
// distance walked over the stride) scaled by the instance's sway amplitude (0 = standing).
//
// Shadows: agents receive the sun's cascaded shadows (BUG-063), and cast their own into the
// same shadow maps (BUG-094): `vertex_depth` is a depth-only draw of the instances in each
// cascade's shadow pass, so the ground filters their shadows like the buildings'.
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

fn world_of(v: Vertex) -> vec3<f32> {
    let sway = v.color_sway.w;
    let phase = v.scale_phase.w;
    var q = v.position;
    // Limbs (alpha = tint + 2 * limb, 1 = leg, 2 = arm): swing about the hip / shoulder,
    // opposite sides in opposite phase, arms against legs. Standing (sway 0): no swing.
    let limb = floor(v.color.a * 0.5 + 0.01);
    if (limb > 0.5 && sway > 0.0) {
        let arm = limb > 1.5;
        let pivot = select(0.8, 1.4, arm);
        let side = select(-1.0, 1.0, (q.x > 0.0) == arm);
        let ang = sin(phase) * 0.6 * side;
        let dy = q.y - pivot;
        let z = q.z * cos(ang) - dy * sin(ang);
        q.y = pivot + dy * cos(ang) + q.z * sin(ang);
        q.z = z;
    }
    var p = q * v.scale_phase.xyz;
    // Gait: sideways sway grows with height, plus a small bob.
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
    let tint = v.color.a - 2.0 * floor(v.color.a * 0.5 + 0.01);
    out.color = v.color.rgb * mix(vec3<f32>(1.0), v.color_sway.rgb, tint);
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

// The model into a cascade's shadow map; the view is the cascade's (the shadow pass binds it
// at group 0, as for Bevy's own casters). Points nearer the sun than the cascade's near plane
// clamp onto it where the pipeline cannot leave depth unclipped.
@vertex
fn vertex_depth(v: Vertex) -> @builtin(position) vec4<f32> {
    var clip = view.clip_from_world * vec4<f32>(world_of(v), 1.0);
    clip.z = min(clip.z, clip.w);
    return clip;
}
