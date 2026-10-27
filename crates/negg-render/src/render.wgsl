// SPDX-License-Identifier: MPL-2.0

// The GPU preview kernel (feature `gpu`, ADR 0036): steps 3 to 7 of
// docs/MATHS.md for one pixel, the same steps in the same order as the fused
// CPU kernel `render_in_place` in full.rs, which stays the reference. Every
// function below names the Rust function it mirrors. `positive` writes the
// linear Rec.2020 positive (compared against the CPU in tests with a
// tolerance of 1/4096 per channel); `display` also encodes it for the screen
// as Little-CMS encodes 8-bit sRGB (step 8 preview), packed as RGBA8.

// The linear input, interleaved RGB.
@group(0) @binding(0) var<storage, read> src: array<f32>;
// Parameters, filled by `params` in gpu.rs, as words: read through `p` as
// floats, except the pixel count and the row width, which are integers.
//  0 dmin.xyz           1 gamma.xyz          2 neutral offset.xyz
//  3 cast (slope_r, offset_r, slope_b, offset_b), identity when unusable
//  4 x0_total.xyz, k    5 (dmin_p, dmax_p, a_hl, a_sh)
//  6 (floor, bpc denominator, saturation, bpc as 0 or 1)
//  7 (density model, matrix, saturation step, all 0 or 1; pixel count)
//  8 density model a.xyz       9 to 11 density model g rows
// 12 to 14 entry matrix rows   15 to 17 display matrix rows
// 18 (pixels per dispatch row, unused, unused, unused)
@group(0) @binding(1) var<uniform> q: array<vec4<u32>, 19>;

// Parameter slot `i` as floats.
fn p(i: u32) -> vec4<f32> {
    return bitcast<vec4<f32>>(q[i]);
}
// The positive, interleaved RGB (entry point `positive`).
@group(0) @binding(2) var<storage, read_write> out_positive: array<f32>;
// The display encoding, one RGBA8 pixel per word (entry point `display`).
@group(0) @binding(3) var<storage, read_write> out_display: array<u32>;

// log10(2) and log2(10).
const LOG10_2: f32 = 0.30102999566398120;
const LOG2_10: f32 = 3.32192809488736235;
// negg_film::grid::TRANSMITTANCE_FLOOR.
const TRANSMITTANCE_FLOOR: f32 = 1e-6;
// saturation.rs LUMA (Rec.2020 luminance).
const LUMA: vec3<f32> = vec3<f32>(0.2627, 0.6780, 0.0593);

// negg_film::density_value: D = -log10(max(I, 1e-6)). Written as a select
// so that NaN, like f32::max on the CPU, gives the floor (WGSL leaves max
// with NaN to the implementation).
fn density_value(i: vec3<f32>) -> vec3<f32> {
    let floor = vec3<f32>(TRANSMITTANCE_FLOOR);
    return -log2(select(floor, i, i > floor)) * LOG10_2;
}

// rig.rs density_model_pixel: d_c = a_c + sum_k g[c][k] * (-D_k).
fn density_model(i: vec3<f32>) -> vec3<f32> {
    let l = -density_value(i);
    return p(8u).xyz + vec3<f32>(dot(p(9u).xyz, l), dot(p(10u).xyz, l), dot(p(11u).xyz, l));
}

// print.rs softplus, in f32 in the same stable form as the f64 original.
fn softplus(z: vec3<f32>) -> vec3<f32> {
    return max(z, vec3<f32>(0.0)) + log(vec3<f32>(1.0) + exp(-abs(z)));
}

// print.rs print_pixel.
fn print_curve(e: vec3<f32>) -> vec3<f32> {
    let k = p(4u).w;
    let dmin_p = p(5u).x;
    let dmax_p = p(5u).y;
    let a_hl = p(5u).z;
    let a_sh = p(5u).w;
    let x = p(4u).xyz + e * k;
    let v = vec3<f32>(dmin_p) + softplus(a_hl * (x - vec3<f32>(dmin_p))) / a_hl;
    let pd = vec3<f32>(dmax_p) - softplus(a_sh * (vec3<f32>(dmax_p) - v)) / a_sh;
    var o = exp2(-pd * LOG2_10);
    if (p(6u).w != 0.0) {
        o = max((o - vec3<f32>(p(6u).x)) / p(6u).y, vec3<f32>(0.0));
    }
    return o;
}

// full.rs render_in_place for one pixel.
fn render_pixel(i: vec3<f32>) -> vec3<f32> {
    var d: vec3<f32>;
    if (p(7u).x != 0.0) {
        d = density_model(i);
    } else {
        d = density_value(i);
    }
    // invert.rs invert_pixel, then the neutral offset.
    var e = -max(d - p(0u).xyz, vec3<f32>(0.0)) / p(1u).xyz - p(2u).xyz;
    // cast.rs apply_cast_pixel (an unusable slope arrives as 1 and 0).
    e = vec3<f32>((e.x - p(3u).y) / p(3u).x, e.y, (e.z - p(3u).w) / p(3u).z);
    var o = print_curve(e);
    // matrix.rs apply_matrix_pixel.
    if (p(7u).y != 0.0) {
        o = max(vec3<f32>(dot(p(12u).xyz, o), dot(p(13u).xyz, o), dot(p(14u).xyz, o)), vec3<f32>(0.0));
    }
    // saturation.rs saturate_pixel.
    if (p(7u).z != 0.0) {
        let y = dot(LUMA, o);
        o = max(vec3<f32>(y) + p(6u).z * (o - vec3<f32>(y)), vec3<f32>(0.0));
    }
    return o;
}

// The IEC 61966-2-1 curve, then Little-CMS's packing of a float pipeline
// into 8 bits: rounded to 16 bits, then FROM_16_TO_8 (negg-cms tests this
// model against OutputTransform::apply_u8).
fn srgb_u8(v: f32) -> u32 {
    var e: f32;
    if (v <= 0.0031308) {
        e = 12.92 * v;
    } else {
        e = 1.055 * pow(v, 1.0 / 2.4) - 0.055;
    }
    let w = u32(clamp(floor(e * 65535.0 + 0.5), 0.0, 65535.0));
    return (w * 65281u + 8388608u) >> 24u;
}

fn pixel_index(id: vec3<u32>) -> u32 {
    return id.x + id.y * q[18].x;
}

fn load(n: u32) -> vec3<f32> {
    return vec3<f32>(src[3u * n], src[3u * n + 1u], src[3u * n + 2u]);
}

@compute @workgroup_size(256)
fn positive(@builtin(global_invocation_id) id: vec3<u32>) {
    let n = pixel_index(id);
    if (n >= q[7].w) {
        return;
    }
    let o = render_pixel(load(n));
    out_positive[3u * n] = o.x;
    out_positive[3u * n + 1u] = o.y;
    out_positive[3u * n + 2u] = o.z;
}

@compute @workgroup_size(256)
fn display(@builtin(global_invocation_id) id: vec3<u32>) {
    let n = pixel_index(id);
    if (n >= q[7].w) {
        return;
    }
    let o = render_pixel(load(n));
    let v = vec3<f32>(dot(p(15u).xyz, o), dot(p(16u).xyz, o), dot(p(17u).xyz, o));
    out_display[n] = srgb_u8(v.x) | (srgb_u8(v.y) << 8u) | (srgb_u8(v.z) << 16u) | (255u << 24u);
}
