// Lit, texture-free material: base colour comes from the mesh's vertex colours.
//
// StandardMaterial binds half a dozen textures per draw. On WebGL2 (wgpu's GLES backend)
// each of those is several GL state calls, which made a 257-draw terrain cost > 1 s per
// frame in Chrome. This material binds one small uniform instead.
import bevy_pbr::render::{
    forward_io::{VertexOutput, FragmentOutput},
    pbr_functions::{apply_pbr_lighting, main_pass_post_lighting_processing},
    pbr_fragment::pbr_input_from_vertex_output,
};

// x = perceptual roughness, y = reflectance
@group(constants::MATERIAL_BIND_GROUP) @binding(0) var<uniform> params: vec4<f32>;

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    var pbr_input = pbr_input_from_vertex_output(in, is_front, false);
    pbr_input.material.perceptual_roughness = params.x;
    pbr_input.material.reflectance = vec3<f32>(params.y);
    var out: FragmentOutput;
    out.color = apply_pbr_lighting(pbr_input);
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
    return out;
}
