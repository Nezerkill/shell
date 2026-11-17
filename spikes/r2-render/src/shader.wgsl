// r2-render: synthetic shell frame -- background, dual-pass separable blur,
// SDF rounded panel + soft shadow, premultiplied composite.
// All uniforms are packed into vec4 lanes so Rust layouts are trivial to check.

struct BgU {
    res: vec4f,    // xy = frame size in px
};

struct BlurU {
    size: vec4f,   // xy = source texture size in px
    dir: vec4f,    // xy = tap step in texels, e.g. (1,0) or (0,1)
    params: vec4f, // x = radius (taps per side), y = sigma in px
};

struct ShadowU {
    size: vec4f,   // xy = layer size in px
    rect: vec4f,   // xy = panel centre in layer px, zw = half size
    params: vec4f, // x = radius, y = softness px, z = opacity, w = vertical offset px
};

struct PanelU {
    layer: vec4f,  // xy = layer size in px
    rect: vec4f,   // xy = panel centre in layer px, zw = half size
    offset: vec4f, // xy = layer origin in frame px, zw = frame size px
    params: vec4f, // x = radius, y = opacity, z = border strength
    tint: vec4f,   // rgb = tint, a = tint mix
};

struct CompU {
    origin: vec4f, // xy = layer origin in frame px, zw = 1 / layer size (target is a WGSL keyword)
};

// Full-screen triangle: the only vertex stage in the spike.
@vertex
fn vs_full(@builtin(vertex_index) vi: u32) -> @builtin(position) vec4f {
    var pos = array<vec2f, 3>(vec2f(-1.0, -1.0), vec2f(3.0, -1.0), vec2f(-1.0, 3.0));
    return vec4f(pos[vi], 0.0, 1.0);
}

// ---------------------------------------------------------------- background
@group(0) @binding(0) var<uniform> bg_u: BgU;

fn hash21(p: vec2f) -> f32 {
    var q = fract(p * vec2f(0.1031, 0.1030));
    q += dot(q, q.yx + 33.33);
    return fract((q.x + q.y) * q.x);
}

@fragment
fn fs_bg(@builtin(position) pos: vec4f) -> @location(0) vec4f {
    let uv = pos.xy / max(bg_u.res.xy, vec2f(1.0));
    var col = mix(
        vec3f(0.030, 0.045, 0.085),
        vec3f(0.170, 0.080, 0.240),
        smoothstep(0.0, 1.0, uv.y),
    );
    let d = distance(uv * vec2f(1.7778, 1.0), vec2f(0.62, 0.30));
    col += vec3f(0.090, 0.110, 0.200) * exp(-d * d * 5.0);
    col += 0.050 * vec3f(0.55, 0.65, 1.0) * (0.5 + 0.5 * sin(d * 70.0));
    // fine structure so that a blur has something to hide
    let px = pos.xy;
    let chk = f32((i32(px.x) / 4 + i32(px.y) / 4) & 1);
    col *= 0.90 + 0.10 * chk;
    let stripe = f32((i32(px.x) / 13) & 1);
    col *= 0.95 + 0.05 * stripe;
    col += (hash21(px) - 0.5) * (1.0 / 255.0); // dither against 8-bit banding
    return vec4f(clamp(col, vec3f(0.0), vec3f(1.0)), 1.0);
}

// --------------------------------------------------------------- blur chain
@group(0) @binding(0) var<uniform> bl_u: BlurU;
@group(0) @binding(1) var src_tex: texture_2d<f32>;

fn src_at(px: vec2i) -> vec3f {
    let sz = vec2i(i32(bl_u.size.x), i32(bl_u.size.y));
    let p = clamp(px, vec2i(0, 0), sz - vec2i(1, 1));
    return textureLoad(src_tex, p, 0).rgb;
}

// 2x2 box downsample, exact texel loads (no filtering): the CPU reference can
// reproduce this bit-for-bit instead of hoping for the same bilinear rounding.
@fragment
fn fs_blur_ds(@builtin(position) pos: vec4f) -> @location(0) vec4f {
    let c = vec2i(i32(pos.x), i32(pos.y)) * 2;
    let acc = src_at(c)
        + src_at(c + vec2i(1, 0))
        + src_at(c + vec2i(0, 1))
        + src_at(c + vec2i(1, 1));
    return vec4f(acc * 0.25, 1.0);
}

// One separable Gaussian pass. Weights are recomputed per texel from sigma so
// the same code serves both blur directions.
@fragment
fn fs_blur(@builtin(position) pos: vec4f) -> @location(0) vec4f {
    let c = vec2i(i32(pos.x), i32(pos.y));
    let dir = vec2i(i32(bl_u.dir.x), i32(bl_u.dir.y));
    let r = i32(bl_u.params.x);
    let sigma = max(bl_u.params.y, 0.5);
    var acc = vec3f(0.0);
    var wsum = 0.0;
    for (var i = -r; i <= r; i = i + 1) {
        let w = exp(-0.5 * f32(i * i) / (sigma * sigma));
        acc += src_at(c + dir * i) * w;
        wsum += w;
    }
    return vec4f(acc / wsum, 1.0);
}

// ---------------------------------------------------------------------- SDF
fn sd_round_box(p: vec2f, half: vec2f, r: f32) -> f32 {
    let rr = clamp(r, 0.0, min(half.x, half.y));
    let q = abs(p) - half + vec2f(rr);
    return length(max(q, vec2f(0.0))) + min(max(q.x, q.y), 0.0) - rr;
}

// --------------------------------------------------------------- soft shadow
@group(0) @binding(0) var<uniform> sh_u: ShadowU;

@fragment
fn fs_shadow(@builtin(position) pos: vec4f) -> @location(0) vec4f {
    let p = pos.xy - sh_u.rect.xy + vec2f(0.0, sh_u.params.w);
    let d = sd_round_box(p, sh_u.rect.zw, sh_u.params.x) - 4.0;
    let soft = max(sh_u.params.y, 1.0);
    let a = (1.0 - smoothstep(0.0, soft, d)) * sh_u.params.z;
    if (a <= 0.0) {
        return vec4f(0.0);
    }
    return vec4f(0.0, 0.0, 0.0, a); // premultiplied black
}

// ------------------------------------------------- frosted panel (SDF + blur)
@group(0) @binding(0) var<uniform> pn_u: PanelU;
@group(0) @binding(1) var frost_tex: texture_2d<f32>;
@group(0) @binding(2) var frost_samp: sampler;

@fragment
fn fs_panel(@builtin(position) pos: vec4f) -> @location(0) vec4f {
    let p = pos.xy - pn_u.rect.xy;
    let half = pn_u.rect.zw;
    let d = sd_round_box(p, half, pn_u.params.x);
    let aa = max(fwidth(d), 1.0e-3);
    let cov = clamp(0.5 - d / aa, 0.0, 1.0);
    if (cov <= 0.0) {
        return vec4f(0.0);
    }
    let frame_px = pos.xy + pn_u.offset.xy;
    let uv = frame_px / max(pn_u.offset.zw, vec2f(1.0));
    let frost = textureSample(frost_tex, frost_samp, uv).rgb;
    let base = mix(frost, pn_u.tint.rgb, pn_u.tint.a);
    let border = smoothstep(-2.5, 0.0, d) * pn_u.params.z;
    let a = cov * pn_u.params.y; // alpha == geometric coverage at opacity 1
    return vec4f((base + vec3f(border)) * a, a);
}

// ---------------------------------------------------------------- composite
@group(0) @binding(0) var<uniform> cp_u: CompU;
@group(0) @binding(1) var layer_tex: texture_2d<f32>;
@group(0) @binding(2) var layer_samp: sampler;

@fragment
fn fs_composite(@builtin(position) pos: vec4f) -> @location(0) vec4f {
    let uv = (pos.xy - cp_u.origin.xy) * cp_u.origin.zw;
    if (uv.x < 0.0 || uv.y < 0.0 || uv.x > 1.0 || uv.y > 1.0) {
        return vec4f(0.0);
    }
    return textureSample(layer_tex, layer_samp, uv);
}
