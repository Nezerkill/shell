// R2 spike: does a wgpu frame with a separable blur, SDF rounded corners and a
// soft shadow fit the 144 Hz budget (6.9 ms) at 3840x2160?
//
// Headless: the device is created without any surface, everything renders into
// textures, so no compositor, no window and no 4K scanout is involved.
//
// Usage: r2-render [--frames N] [--no-png]
// Exit codes: 0 = assertions passed, 1 = an assertion failed, 77 = no GPU.

mod check;

use std::time::{Duration, Instant};

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;
use wgpu::*;

use check::{Checker, Img};

// wgpu version this spike was written against (run.sh cross-checks Cargo.lock).
const WGPU_VER: &str = "30.0.2";
/// 144 Hz -> 6.9 ms per frame. The criterion from spikes/README.md.
const BUDGET_MS: f64 = 6.90;

const FRAME_W: u32 = 3840;
const FRAME_H: u32 = 2160;
const WARMUP: usize = 20;
const FRAMES: usize = 200;
/// One timestamp before the frame and one after each of the 7 passes.
const TS_PER_FRAME: usize = 8;

// Blur chain: half-resolution separable Gaussian, 2*R+1 taps per direction.
const BLUR_RADIUS: i32 = 8;
const BLUR_SIGMA: f32 = 6.0;

// Panel geometry in frame pixels (a control-centre sized panel at 4K scale).
const PANEL_CX: f32 = 860.0;
const PANEL_CY: f32 = 1180.0;
const PANEL_HW: f32 = 330.0;
const PANEL_HH: f32 = 470.0;
const PANEL_R: f32 = 28.0;
const PANEL_OPACITY: f32 = 0.86;
const LAYER_PAD: u32 = 64; // room for the soft shadow

const SHADOW_SOFT: f32 = 46.0;
const SHADOW_OPACITY: f32 = 0.55;
const SHADOW_DY: f32 = 10.0;

// ------------------------------------------------------------------ uniforms
// Every struct is a multiple of 16 bytes and laid out as vec4 lanes, matching
// the WGSL declarations one to one.

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct BgU {
    res: [f32; 4],
}

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct BlurU {
    size: [f32; 4],
    dir: [f32; 4],
    params: [f32; 4],
}

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct ShadowU {
    size: [f32; 4],
    rect: [f32; 4],
    params: [f32; 4],
}

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct PanelU {
    layer: [f32; 4],
    rect: [f32; 4],
    offset: [f32; 4],
    params: [f32; 4],
    tint: [f32; 4],
}

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct CompU {
    target: [f32; 4],
}

struct Pipes {
    bg: RenderPipeline,
    ds: RenderPipeline,
    blur: RenderPipeline,
    shadow: RenderPipeline,
    panel: RenderPipeline,
    panel_corner: RenderPipeline,
    comp: RenderPipeline,
}

fn align_up(x: u32, a: u32) -> u32 {
    x.div_ceil(a) * a
}

fn make_tex(
    device: &Device,
    label: &str,
    w: u32,
    h: u32,
    fmt: TextureFormat,
    usage: TextureUsages,
) -> Texture {
    device.create_texture(&TextureDescriptor {
        label: Some(label),
        size: Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: TextureDimension::D2,
        format: fmt,
        usage,
        view_formats: &[],
    })
}

fn view(tex: &Texture) -> TextureView {
    tex.create_view(&TextureViewDescriptor::default())
}

fn uniform<U: Pod>(device: &Device, label: &str, u: &U) -> Buffer {
    device.create_buffer_init(&util::BufferInitDescriptor {
        label: Some(label),
        contents: bytemuck::bytes_of(u),
        usage: BufferUsages::UNIFORM,
    })
}

fn uniform_only_layout(device: &Device, label: &str) -> BindGroupLayout {
    device.create_bind_group_layout(&BindGroupLayoutDescriptor {
        label: Some(label),
        entries: &[BindGroupLayoutEntry {
            binding: 0,
            visibility: ShaderStages::FRAGMENT,
            ty: BindingType::Buffer {
                ty: BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        }],
    })
}

fn uniform_entry(binding: u32) -> BindGroupLayoutEntry {
    BindGroupLayoutEntry {
        binding,
        visibility: ShaderStages::FRAGMENT,
        ty: BindingType::Buffer {
            ty: BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn tex_entry(binding: u32) -> BindGroupLayoutEntry {
    BindGroupLayoutEntry {
        binding,
        visibility: ShaderStages::FRAGMENT,
        ty: BindingType::Texture {
            sample_type: TextureSampleType::Float { filterable: true },
            view_dimension: TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

fn sampler_entry(binding: u32) -> BindGroupLayoutEntry {
    BindGroupLayoutEntry {
        binding,
        visibility: ShaderStages::FRAGMENT,
        ty: BindingType::Sampler(SamplerBindingType::Filtering),
        count: None,
    }
}

fn premultiplied() -> Option<BlendState> {
    Some(BlendState {
        color: BlendComponent {
            src_factor: BlendFactor::One,
            dst_factor: BlendFactor::OneMinusSrcAlpha,
            operation: BlendOperation::Add,
        },
        alpha: BlendComponent {
            src_factor: BlendFactor::One,
            dst_factor: BlendFactor::OneMinusSrcAlpha,
            operation: BlendOperation::Add,
        },
    })
}

fn make_pipeline(
    device: &Device,
    shader: &ShaderModule,
    label: &str,
    fs_entry: &str,
    layout: &BindGroupLayout,
    format: TextureFormat,
    blend: Option<BlendState>,
) -> RenderPipeline {
    let pl = device.create_pipeline_layout(&PipelineLayoutDescriptor {
        label: Some(label),
        bind_group_layouts: &[Some(layout)],
        immediate_size: 0,
    });
    device.create_render_pipeline(&RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(&pl),
        vertex: VertexState {
            module: shader,
            entry_point: Some("vs_full"),
            compilation_options: PipelineCompilationOptions::default(),
            buffers: &[],
        },
        primitive: PrimitiveState::default(),
        depth_stencil: None,
        multisample: MultisampleState::default(),
        fragment: Some(FragmentState {
            module: shader,
            entry_point: Some(fs_entry),
            compilation_options: PipelineCompilationOptions::default(),
            targets: &[Some(ColorTargetState {
                format,
                blend,
                write_mask: ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    })
}

// A render-pass descriptor with one colour attachment. Written as a macro
// because the transient arrays it borrows must live in the caller's statement.
macro_rules! pass_desc {
    ($label:expr, $view:expr, $load:expr) => {
        RenderPassDescriptor {
            label: Some($label),
            color_attachments: &[Some(RenderPassColorAttachment {
                view: $view,
                depth_slice: None,
                resolve_target: None,
                ops: Operations {
                    load: $load,
                    store: StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        }
    };
}

// ------------------------------------------------------------------- scene
struct Scene {
    frame: Texture,
    frame_view: TextureView,
    _ds: Texture,
    ds_view: TextureView,
    _h: Texture,
    h_view: TextureView,
    _v: Texture,
    v_view: TextureView,
    _layer: Texture,
    layer_view: TextureView,
    layer_origin: (u32, u32),
    layer_size: (u32, u32),

    bg_bg: BindGroup,
    ds_bg: BindGroup,
    blur_h_bg: BindGroup,
    blur_v_bg: BindGroup,
    shadow_bg: BindGroup,
    panel_bg: BindGroup,
    comp_bg: BindGroup,
    _keep: Vec<Buffer>,
}

fn layer_geom() -> (u32, u32, u32, u32) {
    let x0 = (PANEL_CX - PANEL_HW) as u32 - LAYER_PAD;
    let y0 = (PANEL_CY - PANEL_HH) as u32 - LAYER_PAD;
    let w = (PANEL_HW * 2.0) as u32 + 2 * LAYER_PAD;
    let h = (PANEL_HH * 2.0) as u32 + 2 * LAYER_PAD;
    (x0, y0, w, h)
}

fn build_scene(
    device: &Device,
    l_bg: &BindGroupLayout,
    l_blur: &BindGroupLayout,
    l_shadow: &BindGroupLayout,
    l_panel: &BindGroupLayout,
    l_comp: &BindGroupLayout,
    sampler: &Sampler,
) -> Scene {
    let usage = TextureUsages::RENDER_ATTACHMENT | TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_SRC;
    let frame = make_tex(
        device,
        "frame-4k",
        FRAME_W,
        FRAME_H,
        TextureFormat::Rgba8Unorm,
        usage,
    );
    let ds = make_tex(
        device,
        "blur-ds",
        FRAME_W / 2,
        FRAME_H / 2,
        TextureFormat::Rgba8Unorm,
        usage,
    );
    let h = make_tex(
        device,
        "blur-h",
        FRAME_W / 2,
        FRAME_H / 2,
        TextureFormat::Rgba8Unorm,
        usage,
    );
    let v = make_tex(
        device,
        "blur-v",
        FRAME_W / 2,
        FRAME_H / 2,
        TextureFormat::Rgba8Unorm,
        usage,
    );
    let (lx, ly, lw, lh) = layer_geom();
    let layer = make_tex(device, "panel-layer", lw, lh, TextureFormat::Rgba8Unorm, usage);

    let frame_view = view(&frame);
    let ds_view = view(&ds);
    let h_view = view(&h);
    let v_view = view(&v);
    let layer_view = view(&layer);

    let u_bg = uniform(
        device,
        "u-bg",
        &BgU {
            res: [FRAME_W as f32, FRAME_H as f32, 0.0, 0.0],
        },
    );
    let u_ds = uniform(
        device,
        "u-ds",
        &BlurU {
            size: [FRAME_W as f32, FRAME_H as f32, 0.0, 0.0],
            dir: [1.0, 0.0, 0.0, 0.0],
            params: [0.0, 1.0, 0.0, 0.0],
        },
    );
    let u_h = uniform(
        device,
        "u-h",
        &BlurU {
            size: [(FRAME_W / 2) as f32, (FRAME_H / 2) as f32, 0.0, 0.0],
            dir: [1.0, 0.0, 0.0, 0.0],
            params: [BLUR_RADIUS as f32, BLUR_SIGMA, 0.0, 0.0],
        },
    );
    let u_v = uniform(
        device,
        "u-v",
        &BlurU {
            size: [(FRAME_W / 2) as f32, (FRAME_H / 2) as f32, 0.0, 0.0],
            dir: [0.0, 1.0, 0.0, 0.0],
            params: [BLUR_RADIUS as f32, BLUR_SIGMA, 0.0, 0.0],
        },
    );
    let u_shadow = uniform(
        device,
        "u-shadow",
        &ShadowU {
            size: [lw as f32, lh as f32, 0.0, 0.0],
            rect: [PANEL_CX - lx as f32, PANEL_CY - ly as f32, PANEL_HW, PANEL_HH],
            params: [PANEL_R, SHADOW_SOFT, SHADOW_OPACITY, SHADOW_DY],
        },
    );
    let u_panel = uniform(
        device,
        "u-panel",
        &PanelU {
            layer: [lw as f32, lh as f32, 0.0, 0.0],
            rect: [PANEL_CX - lx as f32, PANEL_CY - ly as f32, PANEL_HW, PANEL_HH],
            offset: [lx as f32, ly as f32, FRAME_W as f32, FRAME_H as f32],
            params: [PANEL_R, PANEL_OPACITY, 0.16, 0.0],
            tint: [0.10, 0.12, 0.18, 0.42],
        },
    );
    let u_comp = uniform(
        device,
        "u-comp",
        &CompU {
            target: [lx as f32, ly as f32, 1.0 / lw as f32, 1.0 / lh as f32],
        },
    );

    let bg_bg = device.create_bind_group(&BindGroupDescriptor {
        label: Some("bg"),
        layout: l_bg,
        entries: &[BindGroupEntry {
            binding: 0,
            resource: u_bg.as_entire_binding(),
        }],
    });
    let ds_bg = device.create_bind_group(&BindGroupDescriptor {
        label: Some("blur-ds"),
        layout: l_blur,
        entries: &[
            BindGroupEntry {
                binding: 0,
                resource: u_ds.as_entire_binding(),
            },
            BindGroupEntry {
                binding: 1,
                resource: BindingResource::TextureView(&frame_view),
            },
        ],
    });
    let blur_h_bg = device.create_bind_group(&BindGroupDescriptor {
        label: Some("blur-h"),
        layout: l_blur,
        entries: &[
            BindGroupEntry {
                binding: 0,
                resource: u_h.as_entire_binding(),
            },
            BindGroupEntry {
                binding: 1,
                resource: BindingResource::TextureView(&ds_view),
            },
        ],
    });
    let blur_v_bg = device.create_bind_group(&BindGroupDescriptor {
        label: Some("blur-v"),
        layout: l_blur,
        entries: &[
            BindGroupEntry {
                binding: 0,
                resource: u_v.as_entire_binding(),
            },
            BindGroupEntry {
                binding: 1,
                resource: BindingResource::TextureView(&h_view),
            },
        ],
    });
    let shadow_bg = device.create_bind_group(&BindGroupDescriptor {
        label: Some("shadow"),
        layout: l_shadow,
        entries: &[BindGroupEntry {
            binding: 0,
            resource: u_shadow.as_entire_binding(),
        }],
    });
    let panel_bg = device.create_bind_group(&BindGroupDescriptor {
        label: Some("panel"),
        layout: l_panel,
        entries: &[
            BindGroupEntry {
                binding: 0,
                resource: u_panel.as_entire_binding(),
            },
            BindGroupEntry {
                binding: 1,
                resource: BindingResource::TextureView(&v_view),
            },
            BindGroupEntry {
                binding: 2,
                resource: BindingResource::Sampler(sampler),
            },
        ],
    });
    let comp_bg = device.create_bind_group(&BindGroupDescriptor {
        label: Some("composite"),
        layout: l_comp,
        entries: &[
            BindGroupEntry {
                binding: 0,
                resource: u_comp.as_entire_binding(),
            },
            BindGroupEntry {
                binding: 1,
                resource: BindingResource::TextureView(&layer_view),
            },
            BindGroupEntry {
                binding: 2,
                resource: BindingResource::Sampler(sampler),
            },
        ],
    });

    Scene {
        frame,
        frame_view,
        _ds: ds,
        ds_view,
        _h: h,
        h_view,
        _v: v,
        v_view,
        _layer: layer,
        layer_view,
        layer_origin: (lx, ly),
        layer_size: (lw, lh),
        bg_bg,
        ds_bg,
        blur_h_bg,
        blur_v_bg,
        shadow_bg,
        panel_bg,
        comp_bg,
        _keep: vec![u_bg, u_ds, u_h, u_v, u_shadow, u_panel, u_comp],
    }
}

// One frame: 7 render passes, a timestamp between them when asked.
fn record_frame(enc: &mut CommandEncoder, pipes: &Pipes, sc: &Scene, ts: Option<(&QuerySet, u32)>) {
    macro_rules! mark {
        ($i:expr) => {
            if let Some((q, base)) = ts {
                enc.write_timestamp(q, base + $i);
            }
        };
    }
    mark!(0);
    {
        let mut rp = enc.begin_render_pass(&pass_desc!("bg", &sc.frame_view, LoadOp::Clear(Color::BLACK)));
        rp.set_pipeline(&pipes.bg);
        rp.set_bind_group(0, &sc.bg_bg, &[]);
        rp.draw(0..3, 0..1);
    }
    mark!(1);
    {
        let mut rp = enc.begin_render_pass(&pass_desc!("blur-ds", &sc.ds_view, LoadOp::Clear(Color::BLACK)));
        rp.set_pipeline(&pipes.ds);
        rp.set_bind_group(0, &sc.ds_bg, &[]);
        rp.draw(0..3, 0..1);
    }
    mark!(2);
    {
        let mut rp = enc.begin_render_pass(&pass_desc!("blur-h", &sc.h_view, LoadOp::Clear(Color::BLACK)));
        rp.set_pipeline(&pipes.blur);
        rp.set_bind_group(0, &sc.blur_h_bg, &[]);
        rp.draw(0..3, 0..1);
    }
    mark!(3);
    {
        let mut rp = enc.begin_render_pass(&pass_desc!("blur-v", &sc.v_view, LoadOp::Clear(Color::BLACK)));
        rp.set_pipeline(&pipes.blur);
        rp.set_bind_group(0, &sc.blur_v_bg, &[]);
        rp.draw(0..3, 0..1);
    }
    mark!(4);
    {
        let mut rp = enc.begin_render_pass(&pass_desc!(
            "shadow",
            &sc.layer_view,
            LoadOp::Clear(Color::TRANSPARENT)
        ));
        rp.set_pipeline(&pipes.shadow);
        rp.set_bind_group(0, &sc.shadow_bg, &[]);
        rp.draw(0..3, 0..1);
    }
    mark!(5);
    {
        let mut rp = enc.begin_render_pass(&pass_desc!("panel", &sc.layer_view, LoadOp::Load));
        rp.set_pipeline(&pipes.panel);
        rp.set_bind_group(0, &sc.panel_bg, &[]);
        rp.draw(0..3, 0..1);
    }
    mark!(6);
    {
        let mut rp = enc.begin_render_pass(&pass_desc!("composite", &sc.frame_view, LoadOp::Load));
        rp.set_scissor_rect(
            sc.layer_origin.0,
            sc.layer_origin.1,
            sc.layer_size.0,
            sc.layer_size.1,
        );
        rp.set_pipeline(&pipes.comp);
        rp.set_bind_group(0, &sc.comp_bg, &[]);
        rp.draw(0..3, 0..1);
    }
    mark!(7);
}

// -------------------------------------------------------------- blur test rig
// A small source texture fed through the same downsample/H/V shaders as the
// frame, so the CPU reference has something exactly known to compare against.
struct Rig {
    _src: Texture,
    v: Texture,
    _src_size: (u32, u32),
    ds_size: (u32, u32),
    views: Vec<TextureView>,
    bgs: Vec<BindGroup>,
    _keep: Vec<Buffer>,
}

#[allow(clippy::too_many_arguments)]
fn build_rig(
    device: &Device,
    queue: &Queue,
    l_blur: &BindGroupLayout,
    src_rgba: &[u8],
    w: u32,
    h: u32,
    radius: i32,
    sigma: f32,
) -> Rig {
    let src = make_tex(
        device,
        "rig-src",
        w,
        h,
        TextureFormat::Rgba8Unorm,
        TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
    );
    queue.write_texture(
        TexelCopyTextureInfo {
            texture: &src,
            mip_level: 0,
            origin: Origin3d::ZERO,
            aspect: TextureAspect::All,
        },
        src_rgba,
        TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(w * 4),
            rows_per_image: Some(h),
        },
        Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
    );
    let (dw, dh) = (w / 2, h / 2);
    let usage = TextureUsages::RENDER_ATTACHMENT | TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_SRC;
    let ds = make_tex(device, "rig-ds", dw, dh, TextureFormat::Rgba8Unorm, usage);
    let hz = make_tex(device, "rig-h", dw, dh, TextureFormat::Rgba8Unorm, usage);
    let vt = make_tex(device, "rig-v", dw, dh, TextureFormat::Rgba8Unorm, usage);

    let src_view = view(&src);
    let ds_view = view(&ds);
    let h_view = view(&hz);
    let v_view = view(&vt);

    let u_ds = uniform(
        device,
        "rig-u-ds",
        &BlurU {
            size: [w as f32, h as f32, 0.0, 0.0],
            dir: [1.0, 0.0, 0.0, 0.0],
            params: [0.0, 1.0, 0.0, 0.0],
        },
    );
    let u_h = uniform(
        device,
        "rig-u-h",
        &BlurU {
            size: [dw as f32, dh as f32, 0.0, 0.0],
            dir: [1.0, 0.0, 0.0, 0.0],
            params: [radius as f32, sigma, 0.0, 0.0],
        },
    );
    let u_v = uniform(
        device,
        "rig-u-v",
        &BlurU {
            size: [dw as f32, dh as f32, 0.0, 0.0],
            dir: [0.0, 1.0, 0.0, 0.0],
            params: [radius as f32, sigma, 0.0, 0.0],
        },
    );

    let mk = |label: &str, u: &Buffer, src: &TextureView| {
        device.create_bind_group(&BindGroupDescriptor {
            label: Some(label),
            layout: l_blur,
            entries: &[
                BindGroupEntry {
                    binding: 0,
                    resource: u.as_entire_binding(),
                },
                BindGroupEntry {
                    binding: 1,
                    resource: BindingResource::TextureView(src),
                },
            ],
        })
    };
    let bg_ds = mk("rig-ds", &u_ds, &src_view);
    let bg_h = mk("rig-h", &u_h, &ds_view);
    let bg_v = mk("rig-v", &u_v, &h_view);

    Rig {
        _src: src,
        v: vt,
        _src_size: (w, h),
        ds_size: (dw, dh),
        views: vec![ds_view, h_view, v_view],
        bgs: vec![bg_ds, bg_h, bg_v],
        _keep: vec![u_ds, u_h, u_v],
    }
}

fn run_rig(device: &Device, queue: &Queue, pipes: &Pipes, rig: &Rig) {
    let mut enc = device.create_command_encoder(&CommandEncoderDescriptor { label: Some("rig") });
    for i in 0..3 {
        let mut rp = enc.begin_render_pass(&pass_desc!(
            "rig-pass",
            &rig.views[i],
            LoadOp::Clear(Color::BLACK)
        ));
        rp.set_pipeline(if i == 0 { &pipes.ds } else { &pipes.blur });
        rp.set_bind_group(0, &rig.bgs[i], &[]);
        rp.draw(0..3, 0..1);
    }
    queue.submit(std::iter::once(enc.finish()));
    device.poll(PollType::wait_indefinitely()).expect("poll rig");
}

// ------------------------------------------------------------------ readback
fn read_texture(device: &Device, queue: &Queue, tex: &Texture, w: u32, h: u32, bpp: u32) -> Vec<u8> {
    let unpadded = w * bpp;
    let padded = align_up(unpadded, 256);
    let buf = device.create_buffer(&BufferDescriptor {
        label: Some("readback"),
        size: padded as u64 * h as u64,
        usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = device.create_command_encoder(&CommandEncoderDescriptor {
        label: Some("readback"),
    });
    enc.copy_texture_to_buffer(
        TexelCopyTextureInfo {
            texture: tex,
            mip_level: 0,
            origin: Origin3d::ZERO,
            aspect: TextureAspect::All,
        },
        TexelCopyBufferInfo {
            buffer: &buf,
            layout: TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded),
                rows_per_image: Some(h),
            },
        },
        Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
    );
    queue.submit(std::iter::once(enc.finish()));
    device.poll(PollType::wait_indefinitely()).expect("poll readback");
    let slice = buf.slice(..);
    slice.map_async(MapMode::Read, |_| {});
    device.poll(PollType::wait_indefinitely()).expect("poll map");
    let data = slice.get_mapped_range().expect("get_mapped_range");
    let mut out = Vec::with_capacity(unpadded as usize * h as usize);
    for row in 0..h as usize {
        let start = row * padded as usize;
        out.extend_from_slice(&data[start..start + unpadded as usize]);
    }
    drop(data);
    buf.unmap();
    out
}

fn main() {
    let mut ck = Checker::new();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let no_png = args.iter().any(|a| a == "--no-png");
    let frames: usize = args
        .iter()
        .position(|a| a == "--frames")
        .and_then(|i| args.get(i + 1))
        .and_then(|s| s.parse().ok())
        .unwrap_or(FRAMES);

    println!("== R2: wgpu blur / rounded corners at 4K (offscreen) ==");
    println!("  wgpu {WGPU_VER}, target {FRAME_W}x{FRAME_H} offscreen RGBA8 (no surface, no scanout)");
    println!("  criterion: frame p95 <= {BUDGET_MS:.2} ms (144 Hz)");

    // -- adapter ------------------------------------------------------
    let instance = Instance::new(InstanceDescriptor {
        backends: Backends::VULKAN,
        flags: InstanceFlags::default(),
        memory_budget_thresholds: Default::default(),
        backend_options: Default::default(),
        display: None,
    });
    let adapters = pollster::block_on(instance.enumerate_adapters(Backends::VULKAN));
    if adapters.is_empty() {
        println!(
            "SKIP  no Vulkan adapter available (no ICD / no device); nothing measured, nothing asserted"
        );
        std::process::exit(77);
    }
    let mut chosen = None;
    for a in adapters {
        let i = a.get_info();
        println!(
            "  candidate: {} ({:?}, {:?}) driver {} / {}",
            i.name, i.device_type, i.vendor, i.driver, i.driver_info
        );
        if i.device_type == DeviceType::DiscreteGpu && chosen.is_none() {
            chosen = Some(a);
        }
    }
    let adapter = chosen.expect("no discrete GPU among the Vulkan adapters");
    let info = adapter.get_info();

    // -- features -----------------------------------------------------
    let avail = adapter.features();
    let want_ts = avail.contains(Features::TIMESTAMP_QUERY)
        && avail.contains(Features::TIMESTAMP_QUERY_INSIDE_ENCODERS);
    let mut feats = Features::empty();
    if want_ts {
        feats |= Features::TIMESTAMP_QUERY | Features::TIMESTAMP_QUERY_INSIDE_ENCODERS;
    }
    let (device, queue) = pollster::block_on(adapter.request_device(&DeviceDescriptor {
        label: Some("r2"),
        required_features: feats,
        required_limits: adapter.limits(),
        ..Default::default()
    }))
    .expect("request_device");
    let period = queue.get_timestamp_period();
    println!(
        "  using: {} / backend {:?}, timestamp queries {} (timestamp_period {:.1} ns)",
        info.name,
        info.backend,
        if want_ts {
            "ON"
        } else {
            "OFF (unsupported by adapter)"
        },
        period
    );

    // -- pipelines ----------------------------------------------------
    let shader = device.create_shader_module(ShaderModuleDescriptor {
        label: Some("r2-shader"),
        source: ShaderSource::Wgsl(include_str!("shader.wgsl").into()),
    });
    let l_bg = uniform_only_layout(&device, "l-bg");
    let l_blur = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
        label: Some("l-blur"),
        entries: &[uniform_entry(0), tex_entry(1)],
    });
    let l_shadow = uniform_only_layout(&device, "l-shadow");
    let l_panel = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
        label: Some("l-panel"),
        entries: &[uniform_entry(0), tex_entry(1), sampler_entry(2)],
    });
    let l_comp = l_panel.clone();

    let rgba = TextureFormat::Rgba8Unorm;
    let pipes = Pipes {
        bg: make_pipeline(&device, &shader, "p-bg", "fs_bg", &l_bg, rgba, None),
        ds: make_pipeline(&device, &shader, "p-ds", "fs_blur_ds", &l_blur, rgba, None),
        blur: make_pipeline(&device, &shader, "p-blur", "fs_blur", &l_blur, rgba, None),
        shadow: make_pipeline(
            &device,
            &shader,
            "p-shadow",
            "fs_shadow",
            &l_shadow,
            rgba,
            premultiplied(),
        ),
        panel: make_pipeline(
            &device,
            &shader,
            "p-panel",
            "fs_panel",
            &l_panel,
            rgba,
            premultiplied(),
        ),
        // the corner test writes float coverage without blending so NaN and
        // overshoot stay visible instead of being clamped by an 8-bit format
        panel_corner: make_pipeline(
            &device,
            &shader,
            "p-panel-corner",
            "fs_panel",
            &l_panel,
            TextureFormat::Rgba32Float,
            None,
        ),
        comp: make_pipeline(
            &device,
            &shader,
            "p-comp",
            "fs_composite",
            &l_comp,
            rgba,
            premultiplied(),
        ),
    };
    let sampler = device.create_sampler(&SamplerDescriptor {
        label: Some("linear-clamp"),
        mag_filter: FilterMode::Linear,
        min_filter: FilterMode::Linear,
        mipmap_filter: MipmapFilterMode::Nearest,
        address_mode_u: AddressMode::ClampToEdge,
        address_mode_v: AddressMode::ClampToEdge,
        address_mode_w: AddressMode::ClampToEdge,
        ..Default::default()
    });

    let scene = build_scene(&device, &l_bg, &l_blur, &l_shadow, &l_panel, &l_comp, &sampler);

    // -- capture the raw background once (source for the blur reference) --
    let outdir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("out");
    std::fs::create_dir_all(&outdir).ok();
    let bg_u8 = {
        let mut enc = device.create_command_encoder(&CommandEncoderDescriptor {
            label: Some("bg-capture"),
        });
        {
            let mut rp = enc.begin_render_pass(&pass_desc!(
                "bg-only",
                &scene.frame_view,
                LoadOp::Clear(Color::BLACK)
            ));
            rp.set_pipeline(&pipes.bg);
            rp.set_bind_group(0, &scene.bg_bg, &[]);
            rp.draw(0..3, 0..1);
        }
        queue.submit(std::iter::once(enc.finish()));
        device.poll(PollType::wait_indefinitely()).expect("poll bg");
        read_texture(&device, &queue, &scene.frame, FRAME_W, FRAME_H, 4)
    };

    // ================================================================
    // [1] blur vs CPU reference
    // ================================================================
    println!();
    println!("== [1] blur vs CPU reference ==");
    const CROP_W: u32 = 1280;
    const CROP_H: u32 = 768;
    let (crop_x, crop_y) = (1400u32, 604u32);
    let mut crop_bytes = Vec::with_capacity((CROP_W * CROP_H * 4) as usize);
    for y in 0..CROP_H {
        let start = (((crop_y + y) * FRAME_W + crop_x) * 4) as usize;
        crop_bytes.extend_from_slice(&bg_u8[start..start + (CROP_W * 4) as usize]);
    }
    let rig = build_rig(
        &device,
        &queue,
        &l_blur,
        &crop_bytes,
        CROP_W,
        CROP_H,
        BLUR_RADIUS,
        BLUR_SIGMA,
    );
    run_rig(&device, &queue, &pipes, &rig);
    let gpu_blur = read_texture(&device, &queue, &rig.v, rig.ds_size.0, rig.ds_size.1, 4);
    let cpu = check::cpu_blur(
        &crop_bytes,
        CROP_W as usize,
        CROP_H as usize,
        BLUR_RADIUS,
        BLUR_SIGMA,
    );

    let mut abs_sum = 0f64;
    let mut abs_max = 0i32;
    let mut n = 0u64;
    for (i, px) in cpu.rgb.iter().enumerate() {
        for c in 0..3 {
            let d = (gpu_blur[i * 4 + c] as i32 - px[c] as i32).abs();
            abs_sum += d as f64;
            abs_max = abs_max.max(d);
            n += 1;
        }
    }
    let mae_lsb = abs_sum / n as f64;
    ck.check(
        "blur == CPU separable Gaussian reference",
        mae_lsb <= 0.5 && abs_max <= 2,
        &format!(
            "MAE {mae_lsb:.3} LSB, max {abs_max} LSB over {}x{} px (MAE<=0.5, max<=2)",
            cpu.w, cpu.h
        ),
    );

    // ================================================================
    // [2] flat colour stays flat
    // ================================================================
    println!();
    println!("== [2] flat colour stays flat (no rings / banding) ==");
    let flat_q = [107u8, 79u8, 196u8]; // 0.42 / 0.31 / 0.77 in 8 bit
    let mut flat_src = Vec::with_capacity(256 * 256 * 4);
    for _ in 0..(256 * 256) {
        flat_src.extend_from_slice(&[flat_q[0], flat_q[1], flat_q[2], 255]);
    }
    let rig_flat = build_rig(
        &device,
        &queue,
        &l_blur,
        &flat_src,
        256,
        256,
        BLUR_RADIUS,
        BLUR_SIGMA,
    );
    run_rig(&device, &queue, &pipes, &rig_flat);
    let gpu_flat = read_texture(&device, &queue, &rig_flat.v, 128, 128, 4);
    let mut worst = 0i32;
    for px in gpu_flat.as_chunks::<4>().0 {
        for c in 0..3 {
            worst = worst.max((px[c] as i32 - flat_q[c] as i32).abs());
        }
    }
    ck.check(
        "blur of a flat colour is still flat",
        worst <= 1,
        &format!(
            "max deviation {worst} LSB from [{},{},{}] over 128x128 px (threshold <=1 LSB)",
            flat_q[0], flat_q[1], flat_q[2]
        ),
    );

    // ================================================================
    // [3] blurred hard edge: monotone, no overshoot
    // ================================================================
    println!();
    println!("== [3] blurred hard edge: monotone, no overshoot ==");
    let mut edge_src = Vec::with_capacity(256 * 64 * 4);
    for _y in 0..64 {
        for x in 0..256 {
            let v = if x < 128 { 0u8 } else { 255u8 };
            edge_src.extend_from_slice(&[v, v, v, 255]);
        }
    }
    let rig_edge = build_rig(
        &device,
        &queue,
        &l_blur,
        &edge_src,
        256,
        64,
        BLUR_RADIUS,
        BLUR_SIGMA,
    );
    run_rig(&device, &queue, &pipes, &rig_edge);
    let gpu_edge = read_texture(&device, &queue, &rig_edge.v, 128, 32, 4);
    let row = 16usize;
    let profile: Vec<i32> = (0..128).map(|x| gpu_edge[(row * 128 + x) * 4] as i32).collect();
    let backwards = profile.windows(2).filter(|w| w[1] < w[0]).count();
    let lo = *profile.iter().min().unwrap();
    let hi = *profile.iter().max().unwrap();
    let mids = profile.iter().filter(|v| **v > 25 && **v < 230).count();
    ck.check(
        "edge profile is monotone",
        backwards == 0,
        &format!("{backwards} decreasing steps along the scanline (threshold 0)"),
    );
    ck.check(
        "no overshoot / undershoot at the edge",
        lo >= 0 && hi <= 255 && profile[0] == 0 && profile[127] == 255,
        &format!("range [{lo},{hi}], ends {}/{}", profile[0], profile[127]),
    );
    ck.check(
        "transition is actually blurred",
        mids >= 4,
        &format!("{mids} samples inside (25,230); >=4 expected for a 17-tap Gaussian"),
    );

    // ================================================================
    // [4] rounded-corner SDF, radius 0..32
    // ================================================================
    println!();
    println!("== [4] rounded corners: radius 0..32, coverage in alpha ==");
    const CS: u32 = 256;
    let corner_tex = make_tex(
        &device,
        "corner",
        CS,
        CS,
        TextureFormat::Rgba32Float,
        TextureUsages::RENDER_ATTACHMENT | TextureUsages::COPY_SRC,
    );
    let mut coverage_r16: Vec<f32> = Vec::new();
    for &r in &[0.0f32, 1.0, 2.0, 4.0, 8.0, 16.0, 24.0, 32.0] {
        let u = uniform(
            &device,
            "u-corner",
            &PanelU {
                layer: [CS as f32, CS as f32, 0.0, 0.0],
                rect: [128.0, 128.0, 90.0, 90.0],
                offset: [0.0, 0.0, FRAME_W as f32, FRAME_H as f32],
                params: [r, 1.0, 0.0, 0.0],
                tint: [0.0, 0.0, 0.0, 0.0],
            },
        );
        let bg = device.create_bind_group(&BindGroupDescriptor {
            label: Some("corner"),
            layout: &l_panel,
            entries: &[
                BindGroupEntry {
                    binding: 0,
                    resource: u.as_entire_binding(),
                },
                BindGroupEntry {
                    binding: 1,
                    resource: BindingResource::TextureView(&scene.v_view),
                },
                BindGroupEntry {
                    binding: 2,
                    resource: BindingResource::Sampler(&sampler),
                },
            ],
        });
        let corner_view = view(&corner_tex);
        let mut enc = device.create_command_encoder(&CommandEncoderDescriptor {
            label: Some("corner"),
        });
        {
            let mut rp = enc.begin_render_pass(&pass_desc!(
                "corner",
                &corner_view,
                LoadOp::Clear(Color::TRANSPARENT)
            ));
            rp.set_pipeline(&pipes.panel_corner);
            rp.set_bind_group(0, &bg, &[]);
            rp.draw(0..3, 0..1);
        }
        queue.submit(std::iter::once(enc.finish()));
        device.poll(PollType::wait_indefinitely()).expect("poll corner");
        let raw = read_texture(&device, &queue, &corner_tex, CS, CS, 16);
        let alpha: Vec<f32> = raw
            .as_chunks::<16>()
            .0
            .iter()
            .map(|p| f32::from_le_bytes([p[12], p[13], p[14], p[15]]))
            .collect();
        if (r - 16.0).abs() < 0.5 {
            coverage_r16 = alpha.clone();
        }

        let finite = alpha.iter().all(|a| a.is_finite());
        let in_range = alpha.iter().all(|a| *a >= -1.0e-4 && *a <= 1.0 + 1.0e-4);
        let aa_px = alpha.iter().filter(|a| **a > 0.02 && **a < 0.98).count();
        // diagonal from the panel centre towards the top-left corner
        let mut mono = true;
        let mut prev = 2.0f32;
        let mut mids = 0;
        for s in 0..120usize {
            let idx = (128 - s) * CS as usize + (128 - s);
            let a = alpha[idx];
            if a > prev + 1.0e-5 {
                mono = false;
            }
            if a > 0.02 && a < 0.98 {
                mids += 1;
            }
            prev = a;
        }
        let sum: f64 = alpha.iter().map(|a| *a as f64).sum();
        let area = 180.0 * 180.0 - (4.0 - std::f64::consts::PI) * (r as f64) * (r as f64);
        let area_err = (sum - area).abs() / area;
        // a quarter arc of radius r is ~1.57*r pixels long, so even r=2 must
        // leave a few pixels with fractional coverage
        let aa_ok = if r >= 2.0 { mids >= 1 && aa_px >= 3 } else { true };
        let ok = finite && in_range && mono && aa_ok && area_err < 0.02;
        ck.check(
            &format!("corner coverage r={r:>2}"),
            ok,
            &format!(
                "finite={finite} in[0,1]={in_range} diagonal_monotone={mono} aa_edge_px={aa_px} diagonal_aa_samples={mids} area_err={:.2}%",
                area_err * 100.0
            ),
        );
    }

    // ================================================================
    // timing
    // ================================================================
    println!();
    println!("== timing: {WARMUP} warm-up + {frames} measured frames ==");
    let qs = if want_ts {
        Some(device.create_query_set(&QuerySetDescriptor {
            label: Some("ts"),
            ty: QueryType::Timestamp,
            count: (frames * TS_PER_FRAME) as u32,
        }))
    } else {
        None
    };

    let mut enc_ms: Vec<f64> = Vec::with_capacity(frames);
    let mut sub_ms: Vec<f64> = Vec::with_capacity(frames);
    for i in 0..(WARMUP + frames) {
        let measured = i >= WARMUP;
        let t0 = Instant::now();
        let mut enc = device.create_command_encoder(&CommandEncoderDescriptor { label: Some("frame") });
        let ts = if measured {
            qs.as_ref().map(|q| (q, ((i - WARMUP) * TS_PER_FRAME) as u32))
        } else {
            None
        };
        record_frame(&mut enc, &pipes, &scene, ts);
        let cmd = enc.finish();
        let t1 = Instant::now();
        let idx = queue.submit(std::iter::once(cmd));
        device
            .poll(PollType::Wait {
                submission_index: Some(idx),
                timeout: Some(Duration::from_secs(30)),
            })
            .expect("poll frame");
        let t2 = Instant::now();
        if measured {
            enc_ms.push((t1 - t0).as_secs_f64() * 1e3);
            sub_ms.push((t2 - t1).as_secs_f64() * 1e3);
        }
    }

    let mut pass_ms: Vec<Vec<f64>> = vec![Vec::new(); TS_PER_FRAME - 1];
    let mut total_gpu: Vec<f64> = Vec::new();
    if let Some(q) = &qs {
        let count = (frames * TS_PER_FRAME) as u64;
        // wgpu forbids MAP_READ together with QUERY_RESOLVE, so resolve into a
        // staging buffer and copy into a mappable one.
        let staging = device.create_buffer(&BufferDescriptor {
            label: Some("ts-staging"),
            size: count * 8,
            usage: BufferUsages::QUERY_RESOLVE | BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let buf = device.create_buffer(&BufferDescriptor {
            label: Some("ts-readback"),
            size: count * 8,
            usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc = device.create_command_encoder(&CommandEncoderDescriptor {
            label: Some("resolve-ts"),
        });
        enc.resolve_query_set(q, 0..count as u32, &staging, 0);
        enc.copy_buffer_to_buffer(&staging, 0, &buf, 0, count * 8);
        queue.submit(std::iter::once(enc.finish()));
        device.poll(PollType::wait_indefinitely()).expect("poll resolve");
        let slice = buf.slice(..);
        slice.map_async(MapMode::Read, |_| {});
        device.poll(PollType::wait_indefinitely()).expect("poll ts map");
        let data = slice.get_mapped_range().expect("get_mapped_range ts");
        let ticks: Vec<u64> = data
            .as_chunks::<8>()
            .0
            .iter()
            .map(|c| u64::from_le_bytes(*c))
            .collect();
        drop(data);
        buf.unmap();
        let ns = period as f64;
        for f in 0..frames {
            let base = f * TS_PER_FRAME;
            let mut prev = ticks[base];
            for p in 0..(TS_PER_FRAME - 1) {
                let cur = ticks[base + p + 1];
                pass_ms[p].push(cur.saturating_sub(prev) as f64 * ns / 1.0e6);
                prev = cur;
            }
            total_gpu.push(ticks[base + TS_PER_FRAME - 1].saturating_sub(ticks[base]) as f64 * ns / 1.0e6);
        }
    }

    let names = [
        "background",
        "blur: downsample",
        "blur: horizontal",
        "blur: vertical",
        "shadow",
        "panel (SDF+frost)",
        "composite",
    ];
    println!();
    println!(
        "  {:<20} {:>8} {:>8} {:>8}",
        "pass (GPU, ms)", "p50", "p95", "max"
    );
    for (i, name) in names.iter().enumerate() {
        if pass_ms[i].is_empty() {
            continue;
        }
        println!(
            "  {:<20} {:>8.3} {:>8.3} {:>8.3}",
            name,
            check::pct(&pass_ms[i], 0.5),
            check::pct(&pass_ms[i], 0.95),
            check::max_of(&pass_ms[i])
        );
    }
    let blur_pass: Vec<f64> = (0..frames)
        .map(|f| {
            pass_ms[1].get(f).copied().unwrap_or(0.0)
                + pass_ms[2].get(f).copied().unwrap_or(0.0)
                + pass_ms[3].get(f).copied().unwrap_or(0.0)
        })
        .collect();
    let gpu_p50 = check::pct(&total_gpu, 0.5);
    let gpu_p95 = check::pct(&total_gpu, 0.95);
    let gpu_max = check::max_of(&total_gpu);
    let cpu_p50 = check::pct(&sub_ms, 0.5);
    let cpu_p95 = check::pct(&sub_ms, 0.95);
    let cpu_max = check::max_of(&sub_ms);
    let enc_p50 = check::pct(&enc_ms, 0.5);
    let enc_p95 = check::pct(&enc_ms, 0.95);
    let enc_max = check::max_of(&enc_ms);

    println!();
    println!(
        "  {:<20} {:>8} {:>8} {:>8}",
        "aggregate (ms)", "p50", "p95", "max"
    );
    if !total_gpu.is_empty() {
        println!(
            "  {:<20} {:>8.3} {:>8.3} {:>8.3}",
            "blur chain (GPU)",
            check::pct(&blur_pass, 0.5),
            check::pct(&blur_pass, 0.95),
            check::max_of(&blur_pass)
        );
        println!(
            "  {:<20} {:>8.3} {:>8.3} {:>8.3}",
            "frame total (GPU)", gpu_p50, gpu_p95, gpu_max
        );
    }
    println!(
        "  {:<20} {:>8.3} {:>8.3} {:>8.3}",
        "cpu submit+poll", cpu_p50, cpu_p95, cpu_max
    );
    println!(
        "  {:<20} {:>8.3} {:>8.3} {:>8.3}",
        "cpu encode", enc_p50, enc_p95, enc_max
    );
    if total_gpu.is_empty() {
        println!("  note  no GPU timestamps available: only the CPU wall clock was measured");
    }

    // ================================================================
    // artefacts
    // ================================================================
    println!();
    println!("== artifacts ==");
    if no_png {
        println!("  note  --no-png, skipping PNG dumps");
    } else {
        let mut enc = device.create_command_encoder(&CommandEncoderDescriptor { label: Some("dump") });
        record_frame(&mut enc, &pipes, &scene, None);
        queue.submit(std::iter::once(enc.finish()));
        device.poll(PollType::wait_indefinitely()).expect("poll dump");
        let frame = read_texture(&device, &queue, &scene.frame, FRAME_W, FRAME_H, 4);

        let img = check::downscale(&frame, FRAME_W, FRAME_H, 8);
        let p = outdir.join("frame-4k-1of8.png");
        let n = check::write_png(&p, &img).expect("png");
        println!("  {} ({}x{}, {n} B)", p.display(), img.w, img.h);

        let img = check::crop(&frame, FRAME_W, FRAME_H, 430, 1010, 400, 160);
        let p = outdir.join("blur-edge.png");
        let n = check::write_png(&p, &img).expect("png");
        println!("  {} ({}x{}, {n} B)", p.display(), img.w, img.h);

        let img = check::crop(&frame, FRAME_W, FRAME_H, 1140, 680, 200, 200);
        let p = outdir.join("corner-panel.png");
        let n = check::write_png(&p, &img).expect("png");
        println!("  {} ({}x{}, {n} B)", p.display(), img.w, img.h);

        if !coverage_r16.is_empty() {
            let mut aa = Img::new(160, 160);
            for y in 0..160u32 {
                for x in 0..160u32 {
                    let a = coverage_r16[((20 + y) * CS + (20 + x)) as usize];
                    let g = (a.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
                    aa.set(x, y, [g, g, g, 255]);
                }
            }
            let p = outdir.join("corner-aa-r16.png");
            let n = check::write_png(&p, &aa).expect("png");
            println!("  {} ({}x{}, {n} B)", p.display(), aa.w, aa.h);
        }
    }

    // ================================================================
    // verdict
    // ================================================================
    println!();
    let criterion_ok = if total_gpu.is_empty() {
        cpu_p95 <= BUDGET_MS
    } else {
        gpu_p95 <= BUDGET_MS
    };
    ck.check(
        "CRITERION frame p95 <= 6.9 ms (144 Hz)",
        criterion_ok,
        &format!(
            "gpu_p95={:.3} ms, cpu_p95={:.3} ms, budget={BUDGET_MS:.2} ms",
            if total_gpu.is_empty() { f64::NAN } else { gpu_p95 },
            cpu_p95
        ),
    );
    ck.check(
        "measurement is a 4K offscreen render (not a scanout)",
        true,
        "no surface, no swapchain, no display mode touched",
    );

    println!();
    println!(
        "CRITERION p95_gpu_ms={:.4} p95_cpu_ms={:.4} budget_ms={:.2} gpu_p50_ms={:.4} frames={frames}",
        if total_gpu.is_empty() { f64::NAN } else { gpu_p95 },
        cpu_p95,
        BUDGET_MS,
        if total_gpu.is_empty() { f64::NAN } else { gpu_p50 }
    );
    println!("RESULT: {} checks, {} failed", ck.taken, ck.failed);
    if ck.failed > 0 {
        std::process::exit(1);
    }
}
