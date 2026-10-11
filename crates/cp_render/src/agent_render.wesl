// Instanced vehicles and pedestrians. One draw per (kind, detail): the mesh is the model,
// the per-instance vertex buffer places it. WebGL2-safe (plain instance-stepped attributes).
//
// Pedestrians sway in this vertex shader: the head side-steps and the body bobs with the
// per-instance gait phase, scaled by the instance's sway amplitude (0 = standing).
import bevy_pbr::render::mesh_view_bindings::{view, lights};

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
};

@vertex
fn vertex(v: Vertex) -> VertexOutput {
    var p = v.position * v.scale_phase.xyz;
    // Gait: sideways sway grows with height, plus a small bob.
    let sway = v.color_sway.w;
    let phase = v.scale_phase.w;
    p.x += sin(phase) * sway * (v.position.y / 1.8);
    p.y += abs(sin(phase)) * sway * 0.5;

    let c = cos(v.pos_yaw.w);
    let s = sin(v.pos_yaw.w);
    // Yaw about Y with forward = +z: local (x, z) -> world (x c + z s, -x s + z c).
    let world = vec3<f32>(p.x * c + p.z * s, p.y, -p.x * s + p.z * c) + v.pos_yaw.xyz;
    let n = vec3<f32>(v.normal.x * c + v.normal.z * s, v.normal.y, -v.normal.x * s + v.normal.z * c);

    var out: VertexOutput;
    out.clip_position = view.clip_from_world * vec4<f32>(world, 1.0);
    out.normal = n;
    out.color = v.color.rgb * mix(vec3<f32>(1.0), v.color_sway.rgb, v.color.a);
    return out;
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let n = normalize(in.normal);
    let light = lights.directional_lights[0u];
    let diffuse = max(dot(n, light.direction_to_light), 0.0);
    // Same units as the PBR path: illuminance and ambient scaled by the view exposure.
    let radiance = (light.color.rgb * diffuse / 3.14159 + lights.ambient_color.rgb) * view.exposure;
    return vec4<f32>(in.color * radiance, 1.0);
}
