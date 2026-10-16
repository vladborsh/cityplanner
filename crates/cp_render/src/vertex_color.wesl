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

// x = perceptual roughness, y = reflectance; z, w = the same for dark surfaces (asphalt), z = 0
// switches that off
@group(constants::MATERIAL_BIND_GROUP) @binding(0) var<uniform> params: vec4<f32>;

// x = factor on the alpha (1 = as drawn); only visible with an alpha-blended material
// y = overlay dim (0..1): desaturate and darken the city so a data overlay reads (T-183b)
@group(constants::MATERIAL_BIND_GROUP) @binding(1) var<uniform> fade: vec4<f32>;

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    var pbr_input = pbr_input_from_vertex_output(in, is_front, false);
    pbr_input.material.base_color.a = pbr_input.material.base_color.a * fade.x;
    pbr_input.material.perceptual_roughness = params.x;
    pbr_input.material.reflectance = vec3<f32>(params.y);
    if params.z > 0.0 {
        // Asphalt (linear luminance ~0.11) is the only dark thing in the road mesh: it is glossy,
        // the rest (pavement 0.55, kerb 0.42, paint 0.8, tram 0.2) stays matte.
        let lum = dot(pbr_input.material.base_color.rgb, vec3<f32>(0.2126, 0.7152, 0.0722));
        let dark = 1.0 - smoothstep(0.13, 0.17, lum);
        pbr_input.material.perceptual_roughness = mix(params.x, params.z, dark);
        pbr_input.material.reflectance = vec3<f32>(mix(params.y, params.w, dark));
    }
    var out: FragmentOutput;
    out.color = apply_pbr_lighting(pbr_input);
    if fade.y > 0.0 {
        let l = dot(out.color.rgb, vec3<f32>(0.2126, 0.7152, 0.0722));
        let grey = mix(out.color.rgb, vec3<f32>(l), 0.75 * fade.y);
        out.color = vec4<f32>(grey * (1.0 - 0.4 * fade.y), out.color.a);
    }
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
    return out;
}
