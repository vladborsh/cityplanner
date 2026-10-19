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
import bevy_pbr::render::mesh_view_bindings::globals;

// x = perceptual roughness, y = reflectance; z, w = the same for dark surfaces (asphalt), z = 0
// switches that off
@group(constants::MATERIAL_BIND_GROUP) @binding(0) var<uniform> params: vec4<f32>;

// x = factor on the alpha (1 = as drawn); only visible with an alpha-blended material
// y = overlay dim (0..1): desaturate and darken the city so a data overlay reads (T-183b)
// z = procedural surface detail (T-238): 0 = none, 1 = roads (asphalt grain, patches and cracks,
//     paving tiles on the sidewalks), 2 = ground (grass patches); faded out where a pixel covers
//     more than the detail's size, so nothing shimmers in the distance
// w = cloud shadows (T-272): how much a passing cloud darkens the lit surface (0 = off)
@group(constants::MATERIAL_BIND_GROUP) @binding(1) var<uniform> fade: vec4<f32>;

// Lit windows (T-245): x = night factor (0 day .. 1 night), y = lateness of the night (0 at 19:00
// .. 1 at 04:00), z = intensity, w = 1 when the vertex alpha holds window glow codes (buildings)
@group(constants::MATERIAL_BIND_GROUP) @binding(2) var<uniform> glow: vec4<f32>;

// Lamp colours, the same as `building::lights::TINTS`.
fn tint_of(t: u32) -> vec3<f32> {
    return select(
        select(
            select(vec3<f32>(0.78, 0.85, 1.0), vec3<f32>(1.0, 0.72, 0.42), t == 2u),
            vec3<f32>(1.0, 0.60, 0.28),
            t == 1u,
        ),
        vec3<f32>(1.0, 0.50, 0.18),
        t == 0u,
    );
}

// When the windows of a light class go dark (`building::lights::switch_off`): the share on
// until dawn, the start and the length of the lateness range the others switch off in.
fn switch_off(kind: u32) -> vec3<f32> {
    if kind == 0u {
        return vec3<f32>(0.22, 0.15, 0.85);
    } else if kind == 1u {
        return vec3<f32>(0.08, 0.02, 0.40);
    }
    return vec3<f32>(0.70, 0.0, 1.0);
}

fn pcg(v: u32) -> u32 {
    let s = v * 747796405u + 2891336453u;
    let w = ((s >> ((s >> 28u) + 4u)) ^ s) * 277803737u;
    return (w >> 22u) ^ w;
}

fn unit(h: u32) -> f32 {
    return f32(h >> 8u) / 16777216.0;
}

// Where a surface stands, for the window patterns: `uy` = metres along the wall and up,
// `d` = their change per pixel, `face` = which way the wall faces (16 sectors), `vertical`.
struct Facade {
    uy: vec2<f32>,
    d: vec2<f32>,
    face: u32,
    vertical: bool,
}

// A pattern of windows (T-246, `building::lights::pattern_alpha`): a grid of cells in world
// space, each lit from a hash of it with the building's share and switched off over the night
// as single windows are; where a pixel covers a cell, the average of the cells.
fn pattern_glow(code: u32, fc: Facade) -> vec3<f32> {
    let share = f32(code & 31u) / 31.0;
    let kind = (code >> 5u) & 3u;
    let band = ((code >> 7u) & 1u) == 1u;
    let cell_size = vec2<f32>(f32((code >> 8u) & 15u), f32((code >> 12u) & 15u)) * 0.5;
    let ww = f32((code >> 16u) & 15u) / 15.0;
    let sw = switch_off(kind);
    let still = sw.x + (1.0 - sw.x) * clamp((sw.y + sw.z - glow.y - 0.015) / sw.z, 0.0, 1.0);
    let wh = select(0.5, 1.0, band);
    let average = vec3<f32>(1.0, 0.60, 0.28) * (share * ww * wh * still);
    if !fc.vertical {
        return average;
    }
    let c = fc.uy / cell_size;
    let dc = max(fc.d / cell_size, vec2<f32>(1e-4));
    let cell = floor(c);
    let f = c - cell;
    let h = pcg(bitcast<u32>(i32(cell.x)) ^ pcg(bitcast<u32>(i32(cell.y)) ^ (fc.face * 0x9e3779b9u)));
    var lit = 0.0;
    if unit(h) < share {
        let g = pcg(h);
        let u = unit(g);
        lit = 1.0;
        if u >= sw.x {
            let off = sw.y + sw.z * (u - sw.x) / (1.0 - sw.x);
            lit = 1.0 - smoothstep(off - 0.03, off, glow.y);
        }
        let lo = 0.5 - 0.5 * ww;
        let hi = 0.5 + 0.5 * ww;
        lit = lit * (smoothstep(lo - dc.x, lo + dc.x, f.x) - smoothstep(hi - dc.x, hi + dc.x, f.x));
        if !band {
            lit = lit * (smoothstep(0.3 - dc.y, 0.3 + dc.y, f.y) - smoothstep(0.8 - dc.y, 0.8 + dc.y, f.y));
        }
        let b = g & 255u;
        let t = select(select(select(3u, 2u, b < 218u), 1u, b < 167u), 0u, b < 90u);
        let single = tint_of(t) * lit;
        return mix(single, average, smoothstep(0.5, 1.5, max(dc.x, dc.y)));
    }
    return average * smoothstep(0.5, 1.5, max(dc.x, dc.y));
}

// Light of a surface from its vertex alpha `2 + code`: a window (code below 2^14: off (6 bits,
// 63 = all night) | tint (2 bits) << 6 | square root of the strength (5 bits) << 8) or a
// pattern of windows (bit 20). The same as `building::lights::glow`.
fn window_glow(a: f32, fc: Facade) -> vec3<f32> {
    if a < 1.5 || glow.x <= 0.0 {
        return vec3<f32>(0.0);
    }
    let code = u32(round(a - 2.0));
    if (code & (1u << 20u)) != 0u {
        return pattern_glow(code, fc) * (glow.x * glow.z);
    }
    let q = code & 63u;
    let r = f32((code >> 8u) & 31u) / 31.0;
    let s = r * r;
    var on = 1.0;
    if q != 63u {
        let off = f32(q) / 62.0;
        on = 1.0 - smoothstep(off - 0.03, off, glow.y);
    }
    return tint_of((code >> 6u) & 3u) * (s * on * glow.x * glow.z);
}

// --- Procedural detail (T-238). World-space value noise on the ground plane (x, z).

fn hash2(p: vec2<f32>) -> f32 {
    var q = fract(p * vec2<f32>(0.1031, 0.1030));
    q = q + dot(q, q.yx + 33.33);
    return fract((q.x + q.y) * q.x);
}

fn vnoise(p: vec2<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    let a = hash2(i);
    let b = hash2(i + vec2<f32>(1.0, 0.0));
    let c = hash2(i + vec2<f32>(0.0, 1.0));
    let d = hash2(i + vec2<f32>(1.0, 1.0));
    return mix(mix(a, b, u.x), mix(c, d, u.x), u.y);
}

// Two octaves, 0..1.
fn fbm(p: vec2<f32>) -> f32 {
    return 0.65 * vnoise(p) + 0.35 * vnoise(p * 2.03 + vec2<f32>(17.1, 5.3));
}

// 1 while a pixel covers well under `size` metres, fading to 0 by the time it covers half of it.
fn keep(size: f32, footprint: f32) -> f32 {
    return 1.0 - smoothstep(0.15 * size, 0.5 * size, footprint);
}

// Slope (d/dx, d/dz) of `fbm(p * freq)` per metre, by central differences.
fn fbm_slope(p: vec2<f32>, freq: f32) -> vec2<f32> {
    let e = 0.25 / freq;
    let dx = fbm((p + vec2<f32>(e, 0.0)) * freq) - fbm((p - vec2<f32>(e, 0.0)) * freq);
    let dz = fbm((p + vec2<f32>(0.0, e)) * freq) - fbm((p - vec2<f32>(0.0, e)) * freq);
    return vec2<f32>(dx, dz) / (2.0 * e);
}

fn tilt(n: vec3<f32>, slope: vec2<f32>) -> vec3<f32> {
    return normalize(n - vec3<f32>(slope.x, 0.0, slope.y));
}

struct Detail {
    albedo: f32,
    roughness: f32,
    slope: vec2<f32>,
}

fn asphalt(p: vec2<f32>, fp: f32) -> Detail {
    var d: Detail;
    let grain = fbm(p * 3.0);
    let k_grain = keep(0.33, fp);
    // Repaired patches: blotches a few metres across, a shade darker and less glossy.
    let patch_mask = smoothstep(0.66, 0.70, fbm(p * 0.13 + vec2<f32>(31.0, 7.0)));
    // Cracks: thin lines where a noise crosses one half, only in some places.
    let c = abs(vnoise(p * 0.55 + vec2<f32>(3.0, 11.0)) - 0.5);
    let crack = (1.0 - smoothstep(0.004, 0.012, c))
        * smoothstep(0.55, 0.75, vnoise(p * 0.07)) * keep(0.25, fp);
    let wear = fbm(p * 0.35);
    d.albedo = 1.0 + 0.16 * (grain - 0.5) * k_grain + 0.10 * (wear - 0.5)
        - 0.09 * patch_mask - 0.35 * crack;
    d.roughness = 0.12 * (grain - 0.5) * k_grain + 0.12 * patch_mask + 0.2 * crack;
    d.slope = 0.05 * fbm_slope(p, 3.0) * k_grain;
    return d;
}

fn paving(p: vec2<f32>, fp: f32) -> Detail {
    var d: Detail;
    let size = 1.0;
    let q = p / size;
    let cell = floor(q);
    let g = fract(q);
    // Distance to the nearest joint, metres, and which way it lies.
    let to_x = min(g.x, 1.0 - g.x) * size;
    let to_z = min(g.y, 1.0 - g.y) * size;
    let joint = (1.0 - smoothstep(0.025, 0.06, min(to_x, to_z))) * keep(0.3, fp);
    let tile = hash2(cell + vec2<f32>(91.0, 13.0));
    d.albedo = 1.0 + (0.12 * (tile - 0.5) + 0.05 * (fbm(p * 4.0) - 0.5)) * keep(1.0, fp)
        - 0.28 * joint;
    d.roughness = 0.05 * joint;
    // Tiles bevel down into the joints.
    var s = vec2<f32>(0.0);
    if to_x < to_z {
        s.x = select(-1.0, 1.0, g.x < 0.5);
    } else {
        s.y = select(-1.0, 1.0, g.y < 0.5);
    }
    d.slope = -0.25 * s * joint;
    return d;
}

fn grass(p: vec2<f32>, fp: f32) -> Detail {
    var d: Detail;
    let big = fbm(p * 0.06);
    let mid = fbm(p * 0.45 + vec2<f32>(5.0, 9.0));
    let fine = vnoise(p * 3.0);
    d.albedo = 1.0 + 0.30 * (big - 0.5) + 0.16 * (mid - 0.5) * keep(2.2, fp)
        + 0.10 * (fine - 0.5) * keep(0.33, fp);
    d.roughness = 0.0;
    d.slope = 0.35 * fbm_slope(p, 0.45) * keep(2.2, fp) + 0.08 * fbm_slope(p, 3.0) * keep(0.33, fp);
    return d;
}

// --- Cloud shadows (T-272). Soft blotches a few hundred metres across on the ground plane,
// drifting slowly with the wind; 1 in the open, down to 0 under the middle of a cloud.

// Size of a cloud's shadow, metres (one noise cell), and the wind, metres per second.
const CLOUD_SIZE: f32 = 220.0;
const WIND: vec2<f32> = vec2<f32>(6.0, 2.5);

// `p` turned by about 37 degrees: each octave of the clouds on its own axes, so no square shows.
fn turn37(p: vec2<f32>) -> vec2<f32> {
    return vec2<f32>(0.8 * p.x - 0.6 * p.y, 0.6 * p.x + 0.8 * p.y);
}

fn cloud_cover(p: vec2<f32>) -> f32 {
    // `globals.time` wraps after an hour: the clouds then jump once, like a new weather front.
    let q = (p + WIND * globals.time) / CLOUD_SIZE;
    let q2 = turn37(q) * 2.1 + vec2<f32>(7.3, 1.9);
    let q3 = turn37(q2) * 2.2 + vec2<f32>(2.2, 9.4);
    let n = 0.6 * vnoise(q) + 0.28 * vnoise(q2) + 0.12 * vnoise(q3);
    // Soft edges: a cloud's shadow fades in over tens of metres.
    return smoothstep(0.44, 0.72, n);
}

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    var pbr_input = pbr_input_from_vertex_output(in, is_front, false);
    var emit = vec3<f32>(0.0);
    if glow.w > 0.5 {
        // The alpha is a glow code, not a coverage: the surface is opaque. The derivatives for
        // the window patterns are taken here, outside the branches on the code.
        let n = normalize(in.world_normal);
        let along = normalize(vec2<f32>(-n.z, n.x) + vec2<f32>(1e-6, 0.0));
        var fc: Facade;
        fc.uy = vec2<f32>(dot(in.world_position.xz, along), in.world_position.y);
        fc.d = fwidth(fc.uy);
        let sector = (atan2(n.z, n.x) + 3.14159265) * (16.0 / 6.2831853);
        fc.face = u32(clamp(round(sector), 0.0, 16.0)) & 15u;
        fc.vertical = abs(n.y) < 0.5;
        emit = window_glow(pbr_input.material.base_color.a, fc);
        pbr_input.material.base_color.a = 1.0;
    }
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
    if fade.z > 0.5 {
        let wp = in.world_position.xyz;
        let p = wp.xz;
        let fp = max(length(fwidth(p)), 1e-4);
        let base = pbr_input.material.base_color.rgb;
        let lum = dot(base, vec3<f32>(0.2126, 0.7152, 0.0722));
        let chroma = max(base.r, max(base.g, base.b)) - min(base.r, min(base.g, base.b));
        // Only surfaces that face up get the ground-plane detail (not kerbs or skirts).
        let up = smoothstep(0.6, 0.9, pbr_input.N.y);
        var d: Detail;
        d.albedo = 1.0;
        d.roughness = 0.0;
        d.slope = vec2<f32>(0.0);
        if fade.z < 1.5 {
            if lum < 0.15 && chroma < 0.03 {
                d = asphalt(p, fp);
            } else if lum > 0.45 && lum < 0.65 && chroma < 0.05 {
                d = paving(p, fp);
            }
        } else if base.g > base.r && base.g > base.b && chroma > 0.05 {
            d = grass(p, fp);
        } else {
            // Rock, sand, soil: the large patches only.
            d.albedo = 1.0 + 0.18 * (fbm(p * 0.08) - 0.5);
        }
        let albedo = mix(1.0, d.albedo, up);
        pbr_input.material.base_color = vec4<f32>(base * albedo, pbr_input.material.base_color.a);
        pbr_input.material.perceptual_roughness =
            clamp(pbr_input.material.perceptual_roughness + d.roughness * up, 0.05, 1.0);
        pbr_input.N = tilt(pbr_input.N, d.slope * up);
    }
    var out: FragmentOutput;
    out.color = apply_pbr_lighting(pbr_input);
    if fade.w > 0.0 {
        let cover = cloud_cover(in.world_position.xz);
        out.color = vec4<f32>(out.color.rgb * (1.0 - fade.w * cover), out.color.a);
    }
    out.color = vec4<f32>(out.color.rgb + emit, out.color.a);
    if fade.y > 0.0 {
        let l = dot(out.color.rgb, vec3<f32>(0.2126, 0.7152, 0.0722));
        let grey = mix(out.color.rgb, vec3<f32>(l), 0.75 * fade.y);
        out.color = vec4<f32>(grey * (1.0 - 0.4 * fade.y), out.color.a);
    }
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
    return out;
}
