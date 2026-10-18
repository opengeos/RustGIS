//! The GPU map backend: plays a [`rustgis_engine::render::gpu_scene::GpuScene`] inside egui's
//! wgpu render pass (the pattern SpatialOS ships as `VectorCallback`). This module starts
//! with the pure camera/marker math; pipelines and the paint callback land next.
//!
//! `view_proj` maps origin-relative map-unit vertices to clip space and is golden-tested
//! against `Camera::to_screen` — the CPU path's own transform — to 1/100 px.

use rustgis_engine::map::Camera;

/// Clip-space transform for one draw command: vertices are f32 offsets from the command's
/// `origin` (map units), so the f64 `(origin - center)` subtraction happens on the CPU where
/// the precision still is; a vertex's third coordinate is its height above the ground (map
/// units). Matches `Camera::to_screen` for a flat, turned or tilted camera: the ground offsets
/// `(u, v)` from the centre (logical px, turned to the bearing) and the height `up` go to
/// `x = 2u/w`, `y = -2(v cos p - up sin p)/h` over `w' = 1 - (v sin p + up cos p)/d`, where `p`
/// is the pitch and `d` the eye's distance from the centre (`Camera::project_raised`). `w'` is
/// a point's depth (1 at the centre); clip z is `w' - NEAR`, so the near plane cuts what is
/// behind the eye as `Camera::try_to_screen` leaves it out.
pub(crate) fn view_proj(cam: &Camera, w: f64, h: f64, origin: [f64; 2]) -> [[f32; 4]; 4] {
    let a = 1.0 / cam.res;
    let (sb, cb) = if cam.bearing.is_finite() { cam.bearing.to_radians().sin_cos() } else { (0.0, 1.0) };
    let (dx, dy) = (origin[0] - cam.center.x, origin[1] - cam.center.y);
    // Ground offsets and height as linear forms over (x, y, up, 1).
    let u = [a * cb, -a * sb, 0.0, a * (cb * dx - sb * dy)];
    let v = [-a * sb, -a * cb, 0.0, -a * (sb * dx + cb * dy)];
    let up = [0.0, 0.0, a, 0.0];
    let pitch = rustgis_engine::map::camera::normalize_pitch(cam.pitch);
    let (sp, cp, inv_d) = if pitch == 0.0 {
        (0.0, 1.0, 0.0)
    } else {
        let (s, c) = pitch.to_radians().sin_cos();
        (s, c, 2.0 * rustgis_engine::map::camera::TAN_HALF_FOV / h.max(1e-9))
    };
    let row = |f: &dyn Fn(usize) -> f64| -> [f64; 4] { std::array::from_fn(f) };
    let x = row(&|i| 2.0 / w * u[i]);
    let y = row(&|i| -2.0 / h * (cp * v[i] - sp * up[i]));
    let depth = row(&|i| if i == 3 { 1.0 } else { 0.0 } - inv_d * (sp * v[i] + cp * up[i]));
    let z = row(&|i| depth[i] - if i == 3 { NEAR_DEPTH } else { 0.0 });
    // Column-major, as WGSL's mat4x4f.
    std::array::from_fn(|col| [x[col] as f32, y[col] as f32, z[col] as f32, depth[col] as f32])
}

/// The nearest depth drawn (a fraction of the eye's distance from the centre), as
/// `Camera::try_to_screen` leaves out what is nearer.
const NEAR_DEPTH: f64 = 0.05;

/// Host-side mirror of the marker vertex shader's displacement: a unit-space template point
/// becomes a pixel offset, then a clip offset. Templates are defined y-down (screen), clip y
/// is up — hence the negated y. Test-only: the shader is the production side of the mirror.
#[cfg(test)]
pub(crate) fn marker_displace(template: [f32; 2], radius_px: f32, viewport_px: [f32; 2]) -> [f32; 2] {
    [template[0] * radius_px * 2.0 / viewport_px[0], -template[1] * radius_px * 2.0 / viewport_px[1]]
}

// ---- wgpu playback of a GpuScene (the SpatialOS `VectorCallback` pattern) ----

use std::sync::Arc;

use egui_wgpu::{CallbackResources, CallbackTrait, RenderState, wgpu};
use wgpu::util::DeviceExt;

use rustgis_engine::render::gpu_scene::{CmdClip, DrawCmd, GpuScene, SceneTilt};

/// Vertices per marker shape slot in the template bank (the circle's 24 fan triangles fill it
/// exactly; smaller shapes pad with centre points — zero-area triangles are discarded).
const TEMPLATE_VERTS: usize = 72;
const SHAPES: usize = 5;

/// The five marker templates as triangle-fan lists (unit space, y-down like the CPU path's
/// tiny-skia marker shapes at `render::lib.rs` `Ctx::marker`).
fn marker_templates() -> [[f32; 4]; TEMPLATE_VERTS * SHAPES] {
    let mut out = [[0.0; 4]; TEMPLATE_VERTS * SHAPES];
    // Writes one shape's triangle fan (centre + consecutive outline pairs) into its slot.
    fn fan(outline: &[(f32, f32)], dst: &mut [[f32; 4]]) {
        for (i, &a) in outline.iter().enumerate() {
            let b = outline[(i + 1) % outline.len()];
            for (j, p) in [(0.0f32, 0.0f32), a, b].into_iter().enumerate() {
                if let Some(slot) = dst.get_mut(i * 3 + j) {
                    *slot = [p.0, p.1, 0.0, 0.0];
                }
            }
        }
    }
    // Circle: a 24-gon.
    let circle: Vec<(f32, f32)> = (0..24)
        .map(|i| {
            let a = i as f32 / 24.0 * std::f32::consts::TAU;
            (a.cos(), a.sin())
        })
        .collect();
    fan(&circle, &mut out[0..TEMPLATE_VERTS]);
    fan(&[(-0.9, -0.9), (0.9, -0.9), (0.9, 0.9), (-0.9, 0.9)], &mut out[TEMPLATE_VERTS..2 * TEMPLATE_VERTS]);
    fan(&[(0.0, -1.1), (1.0, 0.75), (-1.0, 0.75)], &mut out[2 * TEMPLATE_VERTS..3 * TEMPLATE_VERTS]);
    fan(&[(0.0, -1.15), (0.9, 0.0), (0.0, 1.15), (-0.9, 0.0)], &mut out[3 * TEMPLATE_VERTS..4 * TEMPLATE_VERTS]);
    let star: Vec<(f32, f32)> = (0..10)
        .map(|i| {
            let a = std::f32::consts::PI * i as f32 / 5.0 - std::f32::consts::FRAC_PI_2;
            let r = if i % 2 == 0 { 1.2 } else { 0.5 };
            (r * a.cos(), r * a.sin())
        })
        .collect();
    fan(&star, &mut out[4 * TEMPLATE_VERTS..5 * TEMPLATE_VERTS]);
    out
}

/// Per-draw-call uniforms (WGSL `CmdUniforms`: 144 bytes).
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct CmdUniforms {
    view_proj: [[f32; 4]; 4],
    /// The depth row of the camera the scene was drawn for: a tilted scene's band keeps to its
    /// own ground ([`CmdClip::depth`]) however the camera has moved since.
    scene_depth: [f32; 4],
    /// The fog a tilted view's ground fades into (premultiplied).
    fog: [f32; 4],
    /// Map units per physical pixel (scales stroke normals); 1 for upright markers; the band's
    /// nearest and farthest depths.
    params: [f32; 4],
    /// Physical pixel size of the map view (scales marker pixel offsets); 1 when tilted (fog,
    /// the reach), and the reach's depth.
    viewport: [f32; 4],
    /// Extrusions: the light's direction (map units, unit length) and how far a shadow falls per
    /// map unit of height (map units), both turned with the view as MapLibre's light is.
    light: [f32; 4],
}

impl CmdUniforms {
    fn new(cam: &Camera, logical_size: [f64; 2], ppp: f32, origin: [f64; 2], tilt: Option<&SceneTilt>, clip: CmdClip) -> Self {
        let [w, h] = logical_size;
        let scene = tilt.map_or(*cam, |t| t.camera);
        let s = view_proj(&scene, w, h, origin);
        CmdUniforms {
            view_proj: view_proj(cam, w, h, origin),
            scene_depth: [s[0][3], s[1][3], s[2][3], s[3][3]],
            fog: tilt.map_or([0.0; 4], |t| t.fog),
            params: [(cam.res / f64::from(ppp)) as f32, if clip.upright { 1.0 } else { 0.0 }, clip.depth[0], clip.depth[1]],
            viewport: [
                (w * f64::from(ppp)) as f32,
                (h * f64::from(ppp)) as f32,
                if cam.pitch != 0.0 { 1.0 } else { 0.0 },
                rustgis_engine::map::camera::FAR as f32,
            ],
            light: extrusion_light(cam),
        }
    }
}

/// The extrusions' light for a camera turned to its bearing (see [`CmdUniforms::light`]):
/// `render::EXTRUSION_LIGHT` is given in the view's ground frame, x right and y down.
fn extrusion_light(cam: &Camera) -> [f32; 4] {
    let (l0, l1, l2) = rustgis_engine::render::EXTRUSION_LIGHT;
    let (s, c) = if cam.bearing.is_finite() { cam.bearing.to_radians().sin_cos() } else { (0.0, 1.0) };
    // The view's frame to the map's (as `Camera::unrotate`, then y up).
    let to_map = |u: f64, v: f64| (u * c - v * s, -(u * s + v * c));
    let (lx, ly) = to_map(l0, l1);
    let len = lx.hypot(ly).max(1e-9);
    let (fx, fy) = to_map(-l0 / l2, -l1 / l2);
    [(lx / len) as f32, (ly / len) as f32, fx as f32, fy as f32]
}

/// The backdrop of a tilted view (WGSL `Backdrop`): water fading into fog toward the reach,
/// and the sky above it.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct BackdropUniforms {
    /// The view's height (logical px), the eye's distance from the centre, the pitch's cosine
    /// and sine.
    tilt: [f32; 4],
    /// Physical pixels per point, the reach's screen row (logical px), the reach's depth.
    params: [f32; 4],
    water: [f32; 4],
    fog: [f32; 4],
    sky: [f32; 4],
}

impl BackdropUniforms {
    fn new(cam: &Camera, logical_size: [f64; 2], ppp: f32, water: [f32; 4], t: &SceneTilt) -> Self {
        let h = logical_size[1];
        let pitch = rustgis_engine::map::camera::normalize_pitch(cam.pitch).to_radians();
        let d = h / 2.0 / rustgis_engine::map::camera::TAN_HALF_FOV;
        let sky_y = cam.sky_y(h).unwrap_or(0.0).max(1e-9);
        BackdropUniforms {
            tilt: [h as f32, d as f32, pitch.cos() as f32, pitch.sin() as f32],
            params: [ppp, sky_y as f32, rustgis_engine::map::camera::FAR as f32, 0.0],
            water,
            fog: t.fog,
            sky: t.sky,
        }
    }
}

/// The scene's depth buffer, for the extrusions, and its stencil, so their shadows darken each
/// pixel once however their pieces overlap.
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth24PlusStencil8;

/// Premultiplied-alpha blending, the same arithmetic tiny-skia composites with.
const PREMULTIPLIED_BLEND: wgpu::BlendState = wgpu::BlendState {
    color: wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::One,
        dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
        operation: wgpu::BlendOperation::Add,
    },
    alpha: wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::One,
        dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
        operation: wgpu::BlendOperation::Add,
    },
};

/// Pipelines, layouts and shared resources, created once per wgpu device. The scene draws
/// into an offscreen 4× MSAA target (anti-aliased like the CPU renderer); its resolved
/// texture is then blitted into egui's 1× pass — paint callbacks share egui's attachment and
/// must match its sample count, so the multisampling happens in our own pass.
pub(crate) struct GpuMapRenderer {
    mesh_pipeline: wgpu::RenderPipeline,
    point_pipeline: wgpu::RenderPipeline,
    image_pipeline: wgpu::RenderPipeline,
    backdrop_pipeline: wgpu::RenderPipeline,
    /// Extruded features' roofs and walls (depth-tested and written), and their shadows.
    extrude_pipeline: wgpu::RenderPipeline,
    shadow_pipeline: wgpu::RenderPipeline,
    blit_pipeline: wgpu::RenderPipeline,
    cmd_layout: wgpu::BindGroupLayout,
    backdrop_layout: wgpu::BindGroupLayout,
    image_layout: wgpu::BindGroupLayout,
    templates: wgpu::Buffer,
    sampler: wgpu::Sampler,
    /// MSAA sample count of the offscreen scene target.
    samples: u32,
}

impl GpuMapRenderer {
    /// Builds the pipelines: the scene pipelines against the offscreen `Rgba8Unorm` MSAA
    /// target, the blit pipeline against egui's own (linear, 1×) target format.
    pub(crate) fn new(rs: &RenderState) -> Self {
        let device = &rs.device;
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("mapgpu"),
            source: wgpu::ShaderSource::Wgsl(include_str!("mapgpu.wgsl").into()),
        });
        let uniform_buf = |visibility: wgpu::ShaderStages, binding: u32, size: u64| wgpu::BindGroupLayoutEntry {
            binding,
            visibility,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: wgpu::BufferSize::new(size),
            },
            count: None,
        };
        let cmd_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("mapgpu_cmd"),
            entries: &[
                uniform_buf(wgpu::ShaderStages::VERTEX_FRAGMENT, 0, std::mem::size_of::<CmdUniforms>() as u64),
                uniform_buf(wgpu::ShaderStages::VERTEX, 1, (TEMPLATE_VERTS * SHAPES * 16) as u64),
            ],
        });
        let backdrop_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("mapgpu_backdrop"),
            entries: &[uniform_buf(wgpu::ShaderStages::FRAGMENT, 0, std::mem::size_of::<BackdropUniforms>() as u64)],
        });
        let image_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("mapgpu_image"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let templates = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("mapgpu_templates"),
            contents: bytemuck::cast_slice(&marker_templates()),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("mapgpu_image"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let pipeline = |label: &str,
                        vs: &str,
                        fs: &str,
                        buffers: &[wgpu::VertexBufferLayout<'_>],
                        layouts: &[&wgpu::BindGroupLayout],
                        format: wgpu::TextureFormat,
                        samples: u32,
                        depth: Option<wgpu::DepthStencilState>| {
            let layouts: Vec<Option<&wgpu::BindGroupLayout>> = layouts.iter().map(|l| Some(*l)).collect();
            let buffers: Vec<Option<wgpu::VertexBufferLayout<'_>>> = buffers.iter().map(|b| Some(b.clone())).collect();
            let target = [Some(wgpu::ColorTargetState { format, blend: Some(PREMULTIPLIED_BLEND), write_mask: wgpu::ColorWrites::ALL })];
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(label),
                bind_group_layouts: &layouts,
                immediate_size: 0,
            });
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some(vs),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    buffers: &buffers,
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(fs),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    targets: &target,
                }),
                primitive: wgpu::PrimitiveState::default(), // triangle list, no culling
                depth_stencil: depth,
                multisample: wgpu::MultisampleState { count: samples, mask: !0, alpha_to_coverage_enabled: false },
                multiview_mask: None,
                cache: None,
            })
        };
        let mesh_buffers = [wgpu::VertexBufferLayout {
            array_stride: 40,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &[
                wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x2, offset: 0, shader_location: 0 },
                wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x2, offset: 8, shader_location: 1 },
                wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x4, offset: 16, shader_location: 2 },
                wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32, offset: 32, shader_location: 3 },
            ],
        }];
        let point_buffers = [wgpu::VertexBufferLayout {
            array_stride: 32,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &[
                wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x2, offset: 0, shader_location: 0 },
                wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x4, offset: 8, shader_location: 1 },
                wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32, offset: 24, shader_location: 2 },
                wgpu::VertexAttribute { format: wgpu::VertexFormat::Uint32, offset: 28, shader_location: 3 },
            ],
        }];
        let extrude_buffers = [wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<rustgis_engine::render::gpu_scene::ExtrudeVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &[
                wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: 0, shader_location: 0 },
                wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x2, offset: 12, shader_location: 1 },
                wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x4, offset: 20, shader_location: 2 },
                wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x4, offset: 36, shader_location: 3 },
            ],
        }];
        let samples = 4;
        let scene_format = wgpu::TextureFormat::Rgba8Unorm;
        // Everything but the extrusions draws in order, ignoring the depth buffer; extrusions test
        // and write it, so the nearest roof or wall shows whatever order they come in.
        let depth = |write: bool, compare: wgpu::CompareFunction| {
            Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(write),
                depth_compare: Some(compare),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            })
        };
        let flat = depth(false, wgpu::CompareFunction::Always);
        // A shadow darkens a pixel only while its stencil is still clear, and marks it.
        let once = wgpu::StencilFaceState {
            compare: wgpu::CompareFunction::Equal,
            fail_op: wgpu::StencilOperation::Keep,
            depth_fail_op: wgpu::StencilOperation::Keep,
            pass_op: wgpu::StencilOperation::IncrementClamp,
        };
        let shadows = flat.clone().map(|d| wgpu::DepthStencilState {
            stencil: wgpu::StencilState { front: once, back: once, read_mask: 0xff, write_mask: 0xff },
            ..d
        });
        let mesh_pipeline =
            pipeline("mapgpu_mesh", "vs_mesh", "fs_mesh", &mesh_buffers, &[&cmd_layout], scene_format, samples, flat.clone());
        let point_pipeline =
            pipeline("mapgpu_points", "vs_marker", "fs_marker", &point_buffers, &[&cmd_layout], scene_format, samples, flat.clone());
        let image_pipeline = pipeline("mapgpu_image", "vs_image", "fs_image", &[], &[&image_layout], scene_format, samples, flat.clone());
        let backdrop_pipeline =
            pipeline("mapgpu_backdrop", "vs_backdrop", "fs_backdrop", &[], &[&backdrop_layout], scene_format, samples, flat.clone());
        let shadow_pipeline =
            pipeline("mapgpu_shadow", "vs_extrude", "fs_extrude", &extrude_buffers, &[&cmd_layout], scene_format, samples, shadows);
        let extrude_pipeline = pipeline(
            "mapgpu_extrude",
            "vs_extrude",
            "fs_extrude",
            &extrude_buffers,
            &[&cmd_layout],
            scene_format,
            samples,
            depth(true, wgpu::CompareFunction::Less),
        );
        // The blit pipeline draws the resolved scene texture into egui's pass (egui's target
        // is 1×, so a separate pipeline; the scene's own pipelines render into the 4× MSAA
        // offscreen target).
        let blit_pipeline = pipeline("mapgpu_blit", "vs_image", "fs_image", &[], &[&image_layout], rs.target_format, 1, None);
        GpuMapRenderer {
            mesh_pipeline,
            point_pipeline,
            image_pipeline,
            backdrop_pipeline,
            extrude_pipeline,
            shadow_pipeline,
            blit_pipeline,
            cmd_layout,
            backdrop_layout,
            image_layout,
            templates,
            sampler,
            samples,
        }
    }
}

/// The offscreen scene target: a 4× MSAA color attachment plus the plain texture it resolves
/// to (the blit source in egui's pass). Recreated when the map view's physical size changes.
pub(crate) struct SceneTarget {
    msaa_view: wgpu::TextureView,
    /// The extrusions' depth buffer, multisampled like the colour.
    depth_view: wgpu::TextureView,
    resolve_tex: wgpu::Texture,
    pub resolve_bind: wgpu::BindGroup,
    pub size: [u32; 2],
}

impl SceneTarget {
    pub(crate) fn new(device: &wgpu::Device, r: &GpuMapRenderer, size: [u32; 2]) -> Option<SceneTarget> {
        let [w, h] = size;
        if w == 0 || h == 0 {
            return None;
        }
        let msaa_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("mapgpu_msaa"),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: r.samples,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let depth_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("mapgpu_depth"),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: r.samples,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let resolve_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("mapgpu_resolve"),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let resolve_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("mapgpu_resolve_bind"),
            layout: &r.image_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&resolve_tex.create_view(&wgpu::TextureViewDescriptor::default())),
                },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&r.sampler) },
            ],
        });
        Some(SceneTarget {
            msaa_view: msaa_tex.create_view(&wgpu::TextureViewDescriptor::default()),
            depth_view: depth_tex.create_view(&wgpu::TextureViewDescriptor::default()),
            resolve_tex,
            resolve_bind,
            size,
        })
    }
}

/// Records the scene's MSAA render pass into `enc` (resolves into `tgt.resolve_tex`).
fn record_scene(r: &GpuMapRenderer, draws: &[SceneDraw], clear: [f32; 4], tgt: &SceneTarget, enc: &mut wgpu::CommandEncoder) {
    let mut rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("mapgpu_scene"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: &tgt.msaa_view,
            depth_slice: None,
            resolve_target: Some(&tgt.resolve_tex.create_view(&wgpu::TextureViewDescriptor::default())),
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color { r: clear[0] as f64, g: clear[1] as f64, b: clear[2] as f64, a: clear[3] as f64 }),
                store: wgpu::StoreOp::Discard,
            },
        })],
        depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
            view: &tgt.depth_view,
            depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Discard }),
            stencil_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(0), store: wgpu::StoreOp::Discard }),
        }),
        ..Default::default()
    });
    paint_draws(r, draws, &mut rp);
}

/// One uploaded draw command.
pub(crate) enum SceneDraw {
    Mesh {
        vertices: wgpu::Buffer,
        indices: wgpu::Buffer,
        index_count: u32,
        uniforms: wgpu::Buffer,
        bind: wgpu::BindGroup,
        origin: [f64; 2],
        clip: CmdClip,
    },
    Points {
        instances: wgpu::Buffer,
        count: u32,
        uniforms: wgpu::Buffer,
        bind: wgpu::BindGroup,
        origin: [f64; 2],
        clip: CmdClip,
    },
    /// Extruded features (depth-tested), or their shadows.
    Extrusion {
        vertices: wgpu::Buffer,
        indices: wgpu::Buffer,
        index_count: u32,
        uniforms: wgpu::Buffer,
        bind: wgpu::BindGroup,
        origin: [f64; 2],
        clip: CmdClip,
        shadows: bool,
    },
    /// A tilted view's water, fog and sky, under everything.
    Backdrop {
        uniforms: wgpu::Buffer,
        bind: wgpu::BindGroup,
    },
    Image {
        bind: wgpu::BindGroup,
        _texture: wgpu::Texture,
    },
}

/// A whole scene on the GPU, plus the view parameters needed to recompute per-draw uniforms
/// for a moved camera (`set_camera`).
pub(crate) struct SceneBuffers {
    pub draws: Vec<SceneDraw>,
    /// The scene's clear colour (the basemap's water), used as the MSAA pass's load op.
    pub clear: [f32; 4],
    pub size: [f64; 2],
    pub ppp: f32,
    /// Scenes containing CPU-rasterized image runs can't fast-path a camera move (images are
    /// screen-space bitmaps of the old view) — they take the full re-render instead.
    pub has_images: bool,
    /// A turned or tilted scene: the camera it was drawn for, its sky and fog.
    pub tilt: Option<SceneTilt>,
}

impl SceneBuffers {
    /// Whether the scene was drawn for a tilted view (with its backdrop and bands): a camera
    /// move may re-project it only to a view tilted (or flat) as well.
    pub(crate) fn tilted(&self) -> bool {
        self.tilt.is_some_and(|t| t.camera.pitch != 0.0)
    }
}

/// Whether one draw command fits the device: each of its buffers within `max_buffer_size`,
/// its index/instance count within u32 (they become draw counts), and images non-empty,
/// within `max_texture_dimension_2d`, with exactly `w*h*4` pixels — anything else would only
/// raise wgpu validation errors (fatal under eframe), so the caller renders the frame with
/// the CPU backend instead.
fn cmd_fits(limits: &wgpu::Limits, cmd: &DrawCmd) -> bool {
    let fits = |bytes: usize| u64::try_from(bytes).is_ok_and(|b| b <= limits.max_buffer_size);
    let max_dim = limits.max_texture_dimension_2d;
    match cmd {
        DrawCmd::Mesh { vertices, indices, .. } => {
            fits(std::mem::size_of_val(vertices.as_slice()))
                && fits(std::mem::size_of_val(indices.as_slice()))
                && u32::try_from(indices.len()).is_ok()
        }
        DrawCmd::Points { instances, .. } => fits(std::mem::size_of_val(instances.as_slice())) && u32::try_from(instances.len()).is_ok(),
        DrawCmd::Extrusion { vertices, indices, .. } => {
            fits(std::mem::size_of_val(vertices.as_slice()))
                && fits(std::mem::size_of_val(indices.as_slice()))
                && u32::try_from(indices.len()).is_ok()
        }
        DrawCmd::Image { pixmap } => {
            let (w, h) = (pixmap.width(), pixmap.height());
            (1..=max_dim).contains(&w) && (1..=max_dim).contains(&h) && pixmap.data().len() as u64 == w as u64 * h as u64 * 4
        }
    }
}

impl SceneBuffers {
    /// Uploads a scene's draw commands (meshes, marker instances, CPU images) and bakes the
    /// initial per-command camera uniforms. `None` when the scene exceeds the device's limits
    /// (`cmd_fits`) — the caller renders it with the CPU backend instead.
    pub(crate) fn from_scene(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        r: &GpuMapRenderer,
        scene: &GpuScene,
        camera: Camera,
        size: [f64; 2],
        ppp: f32,
    ) -> Option<SceneBuffers> {
        let limits = device.limits();
        if !scene.cmds.iter().all(|c| cmd_fits(&limits, c)) {
            return None;
        }
        let mut draws = Vec::with_capacity(scene.cmds.len() + 1);
        let mut has_images = false;
        let tilt = scene.tilt.as_ref();
        if let Some(t) = tilt.filter(|t| t.camera.pitch != 0.0) {
            let u = BackdropUniforms::new(&camera, size, ppp, scene.clear, t);
            let uniforms = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("mapgpu_backdrop_uniforms"),
                contents: bytemuck::bytes_of(&u),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            });
            let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("mapgpu_backdrop_bind"),
                layout: &r.backdrop_layout,
                entries: &[wgpu::BindGroupEntry { binding: 0, resource: uniforms.as_entire_binding() }],
            });
            draws.push(SceneDraw::Backdrop { uniforms, bind });
        }
        for (i, cmd) in scene.cmds.iter().enumerate() {
            let clip = scene.clips.get(i).copied().unwrap_or(CmdClip::ALL);
            match cmd {
                DrawCmd::Mesh { origin, vertices, indices } => {
                    if indices.is_empty() {
                        continue;
                    }
                    let uniforms = CmdUniforms::new(&camera, size, ppp, *origin, tilt, clip);
                    let uniforms = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("mapgpu_cmd_uniforms"),
                        contents: bytemuck::bytes_of(&uniforms),
                        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    });
                    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("mapgpu_mesh_bind"),
                        layout: &r.cmd_layout,
                        entries: &[
                            wgpu::BindGroupEntry { binding: 0, resource: uniforms.as_entire_binding() },
                            wgpu::BindGroupEntry { binding: 1, resource: r.templates.as_entire_binding() },
                        ],
                    });
                    draws.push(SceneDraw::Mesh {
                        vertices: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                            label: Some("mapgpu_mesh_vertices"),
                            contents: bytemuck::cast_slice(vertices),
                            usage: wgpu::BufferUsages::VERTEX,
                        }),
                        indices: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                            label: Some("mapgpu_mesh_indices"),
                            contents: bytemuck::cast_slice(indices),
                            usage: wgpu::BufferUsages::INDEX,
                        }),
                        index_count: indices.len() as u32,
                        uniforms,
                        bind,
                        origin: *origin,
                        clip,
                    });
                }
                DrawCmd::Points { origin, instances } => {
                    if instances.is_empty() {
                        continue;
                    }
                    let uniforms = CmdUniforms::new(&camera, size, ppp, *origin, tilt, clip);
                    let uniforms = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("mapgpu_cmd_uniforms"),
                        contents: bytemuck::bytes_of(&uniforms),
                        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    });
                    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("mapgpu_points_bind"),
                        layout: &r.cmd_layout,
                        entries: &[
                            wgpu::BindGroupEntry { binding: 0, resource: uniforms.as_entire_binding() },
                            wgpu::BindGroupEntry { binding: 1, resource: r.templates.as_entire_binding() },
                        ],
                    });
                    draws.push(SceneDraw::Points {
                        instances: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                            label: Some("mapgpu_point_instances"),
                            contents: bytemuck::cast_slice(instances),
                            usage: wgpu::BufferUsages::VERTEX,
                        }),
                        count: instances.len() as u32,
                        uniforms,
                        bind,
                        origin: *origin,
                        clip,
                    });
                }
                DrawCmd::Extrusion { origin, vertices, indices, shadows } => {
                    if indices.is_empty() {
                        continue;
                    }
                    let uniforms = CmdUniforms::new(&camera, size, ppp, *origin, tilt, clip);
                    let uniforms = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("mapgpu_cmd_uniforms"),
                        contents: bytemuck::bytes_of(&uniforms),
                        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    });
                    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("mapgpu_extrude_bind"),
                        layout: &r.cmd_layout,
                        entries: &[
                            wgpu::BindGroupEntry { binding: 0, resource: uniforms.as_entire_binding() },
                            wgpu::BindGroupEntry { binding: 1, resource: r.templates.as_entire_binding() },
                        ],
                    });
                    draws.push(SceneDraw::Extrusion {
                        vertices: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                            label: Some("mapgpu_extrude_vertices"),
                            contents: bytemuck::cast_slice(vertices),
                            usage: wgpu::BufferUsages::VERTEX,
                        }),
                        indices: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                            label: Some("mapgpu_extrude_indices"),
                            contents: bytemuck::cast_slice(indices),
                            usage: wgpu::BufferUsages::INDEX,
                        }),
                        index_count: indices.len() as u32,
                        uniforms,
                        bind,
                        origin: *origin,
                        clip,
                        shadows: *shadows,
                    });
                }
                DrawCmd::Image { pixmap } => {
                    let texture = device.create_texture(&wgpu::TextureDescriptor {
                        label: Some("mapgpu_image"),
                        size: wgpu::Extent3d { width: pixmap.width(), height: pixmap.height(), depth_or_array_layers: 1 },
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        // Linear, like every egui texture: values pass through unconverted.
                        format: wgpu::TextureFormat::Rgba8Unorm,
                        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                        view_formats: &[],
                    });
                    queue.write_texture(
                        wgpu::TexelCopyTextureInfo {
                            texture: &texture,
                            mip_level: 0,
                            origin: wgpu::Origin3d::ZERO,
                            aspect: wgpu::TextureAspect::All,
                        },
                        pixmap.data(),
                        wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(pixmap.width() * 4),
                            rows_per_image: Some(pixmap.height()),
                        },
                        wgpu::Extent3d { width: pixmap.width(), height: pixmap.height(), depth_or_array_layers: 1 },
                    );
                    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
                    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("mapgpu_image_bind"),
                        layout: &r.image_layout,
                        entries: &[
                            wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
                            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&r.sampler) },
                        ],
                    });
                    draws.push(SceneDraw::Image { bind, _texture: texture });
                    has_images = true;
                }
            }
        }
        Some(SceneBuffers { draws, clear: scene.clear, size, ppp, has_images, tilt: scene.tilt })
    }

    /// Rewrites every command's camera uniforms for a moved camera — pan/zoom without
    /// re-tessellation (meshes re-project in the vertex shader). wgpu buffer writes go through
    /// the queue, so `&self` suffices. Callers check `has_images` first: image runs don't
    /// re-project and need a full re-render.
    pub(crate) fn set_camera(&self, queue: &wgpu::Queue, cam: &Camera) {
        for d in &self.draws {
            let (uniforms, origin, clip) = match d {
                SceneDraw::Mesh { uniforms, origin, clip, .. }
                | SceneDraw::Points { uniforms, origin, clip, .. }
                | SceneDraw::Extrusion { uniforms, origin, clip, .. } => (uniforms, origin, clip),
                SceneDraw::Backdrop { uniforms, .. } => {
                    if let Some(t) = &self.tilt {
                        let u = BackdropUniforms::new(cam, self.size, self.ppp, self.clear, t);
                        queue.write_buffer(uniforms, 0, bytemuck::bytes_of(&u));
                    }
                    continue;
                }
                SceneDraw::Image { .. } => continue,
            };
            let u = CmdUniforms::new(cam, self.size, self.ppp, *origin, self.tilt.as_ref(), *clip);
            queue.write_buffer(uniforms, 0, bytemuck::bytes_of(&u));
        }
    }
}

/// Draws the scene inside egui's render pass (painter's algorithm; no depth).
pub(crate) struct SceneCallback {
    pub renderer: Arc<GpuMapRenderer>,
    pub buffers: Arc<SceneBuffers>,
    /// The offscreen MSAA target, persistent across frames (recreated on size changes).
    pub target: Arc<std::sync::Mutex<Option<SceneTarget>>>,
}

/// The draw loop, shared by the egui paint callback and the offscreen parity harness.
fn paint_draws(renderer: &GpuMapRenderer, draws: &[SceneDraw], rp: &mut wgpu::RenderPass<'_>) {
    for d in draws {
        match d {
            SceneDraw::Mesh { vertices, indices, index_count, bind, .. } => {
                rp.set_pipeline(&renderer.mesh_pipeline);
                rp.set_bind_group(0, bind, &[]);
                rp.set_vertex_buffer(0, vertices.slice(..));
                rp.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
                rp.draw_indexed(0..*index_count, 0, 0..1);
            }
            SceneDraw::Points { instances, count, bind, .. } => {
                rp.set_pipeline(&renderer.point_pipeline);
                rp.set_bind_group(0, bind, &[]);
                rp.set_vertex_buffer(0, instances.slice(..));
                rp.draw(0..TEMPLATE_VERTS as u32, 0..*count);
            }
            SceneDraw::Extrusion { vertices, indices, index_count, bind, shadows, .. } => {
                rp.set_pipeline(if *shadows { &renderer.shadow_pipeline } else { &renderer.extrude_pipeline });
                rp.set_bind_group(0, bind, &[]);
                rp.set_vertex_buffer(0, vertices.slice(..));
                rp.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
                rp.draw_indexed(0..*index_count, 0, 0..1);
            }
            SceneDraw::Backdrop { bind, .. } => {
                rp.set_pipeline(&renderer.backdrop_pipeline);
                rp.set_bind_group(0, bind, &[]);
                rp.draw(0..6, 0..1);
            }
            SceneDraw::Image { bind, .. } => {
                rp.set_pipeline(&renderer.image_pipeline);
                rp.set_bind_group(0, bind, &[]);
                rp.draw(0..6, 0..1);
            }
        }
    }
}

impl CallbackTrait for SceneCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        _queue: &wgpu::Queue,
        _screen_descriptor: &egui_wgpu::ScreenDescriptor,
        egui_encoder: &mut wgpu::CommandEncoder,
        _callback_resources: &mut CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let w = (self.buffers.size[0] * f64::from(self.buffers.ppp)).round() as u32;
        let h = (self.buffers.size[1] * f64::from(self.buffers.ppp)).round() as u32;
        let mut guard = self.target.lock().unwrap_or_else(|e| e.into_inner());
        if guard.as_ref().map(|t| t.size) != Some([w, h]) {
            *guard = SceneTarget::new(device, &self.renderer, [w, h]);
        }
        if let Some(tgt) = guard.as_ref() {
            record_scene(&self.renderer, &self.buffers.draws, self.buffers.clear, tgt, egui_encoder);
        }
        Vec::new()
    }

    fn paint(&self, _info: egui::PaintCallbackInfo, rp: &mut wgpu::RenderPass<'static>, _res: &CallbackResources) {
        let guard = self.target.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(tgt) = guard.as_ref() {
            rp.set_pipeline(&self.renderer.blit_pipeline);
            rp.set_bind_group(0, &tgt.resolve_bind, &[]);
            rp.draw(0..6, 0..1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustgis_engine::geom::Coord;
    use rustgis_engine::render::gpu_scene::{MeshVertex, PointInstance};

    fn assert_matches_to_screen(cam: Camera, w: f64, h: f64, p: Coord, origin: [f64; 2]) {
        let m = view_proj(&cam, w, h, origin);
        let (vx, vy) = ((p.x - origin[0]) as f32, (p.y - origin[1]) as f32);
        let cx = f64::from(m[0][0] * vx + m[3][0]);
        let cy = f64::from(m[1][1] * vy + m[3][1]);
        let (gx, gy) = ((cx + 1.0) * 0.5 * w, (1.0 - cy) * 0.5 * h);
        let (ex, ey) = cam.to_screen(p, w, h);
        assert!((gx - ex).abs() < 0.01 && (gy - ey).abs() < 0.01, "gpu ({gx},{gy}) vs cpu ({ex},{ey})");
    }

    #[test]
    fn matches_to_screen() {
        // World view, a city zoom, and a custom-projection's units (e.g. UTM metres).
        assert_matches_to_screen(Camera::world(), 1280.0, 720.0, Coord::new(2_611_702.0, 8_565_456.0), [0.0, 2_000_000.0]);
        let city = Camera { center: Coord::new(2_000_000.0, 6_000_000.0), res: 150.0, bearing: 0.0, pitch: 0.0 };
        assert_matches_to_screen(city, 1280.0, 720.0, Coord::new(2_013_400.0, 5_991_100.0), [2_000_000.0, 6_000_000.0]);
        let utm = Camera { center: Coord::new(500_000.0, 4_600_000.0), res: 25.0, bearing: 0.0, pitch: 0.0 };
        assert_matches_to_screen(utm, 800.0, 600.0, Coord::new(500_312.0, 4_599_511.0), [500_000.0, 4_600_000.0]);
    }

    #[test]
    fn origin_off_center_shifts_clip() {
        // A vertex 100 units right of the origin, origin 1000 units right of the camera:
        // clip.x = (100 + 1000) * 2/(res*w) = 1100 * 0.002.
        let cam = Camera { center: Coord::new(0.0, 0.0), res: 10.0, bearing: 0.0, pitch: 0.0 };
        let m = view_proj(&cam, 100.0, 100.0, [1000.0, 0.0]);
        let clip_x = f64::from(m[0][0] * 100.0 + m[3][0]);
        assert!((clip_x - 2.2).abs() < 1e-5);
    }

    #[test]
    fn marker_displacement_is_pixels_to_clip() {
        // 4 px radius at a 1280x720 viewport: 8 px of NDC width-2 → 8/1280*2.
        let [dx, dy] = marker_displace([1.0, -1.0], 4.0, [1280.0, 720.0]);
        assert!((dx - 0.00625).abs() < 1e-6);
        assert!(dy > 0.0, "template y is screen-down, clip y is up");
    }

    #[test]
    fn marker_templates_match_cpu_shapes() {
        let t = marker_templates();
        // Circle: 24 fan triangles = 72 vertices filling the slot, on the unit ring or centre.
        assert!(t[1][0].abs() + t[1][1].abs() > 0.9, "circle starts on the unit ring");
        // Square: 4 triangles = 12 vertices, then centre padding.
        let sq = &t[TEMPLATE_VERTS..TEMPLATE_VERTS + 12];
        assert!(sq.iter().any(|p| p[0] == 0.9 && p[1] == 0.9));
        assert!(t[TEMPLATE_VERTS + 12..2 * TEMPLATE_VERTS].iter().all(|p| *p == [0.0; 4]), "padding stays at the centre");
        // Star's top point (y-down): (0, -1.2).
        let star = &t[4 * TEMPLATE_VERTS..5 * TEMPLATE_VERTS];
        assert!(star.iter().any(|p| (p[1] + 1.2).abs() < 1e-6), "star top point");
    }

    /// A headless wgpu render state (the egui_kittest recipe). `None` on hosts without any
    /// adapter — CI without a GPU skips these tests.
    fn headless_render_state() -> Option<RenderState> {
        headless_render_state_on(wgpu::Backends::all())
    }

    /// `backends` selects the wgpu backends; the web app is GL-forced (`apps/rustgis-web`),
    /// so the suite also runs the world sample over GL.
    fn headless_render_state_on(backends: wgpu::Backends) -> Option<RenderState> {
        let setup = egui_wgpu::WgpuSetup::CreateNew({
            let mut s = egui_wgpu::WgpuSetupCreateNew::without_display_handle();
            s.instance_descriptor.backends = backends;
            s
        });
        let instance = pollster::block_on(setup.new_instance());
        pollster::block_on(egui_wgpu::RenderState::create(
            &egui_wgpu::WgpuConfiguration { wgpu_setup: setup, ..Default::default() },
            &instance,
            None,
            egui_wgpu::RendererOptions::PREDICTABLE,
        ))
        .ok()
    }

    #[test]
    fn commands_over_device_limits_are_declined() {
        let limits = wgpu::Limits { max_buffer_size: 1024, max_texture_dimension_2d: 2048, ..Default::default() };
        let mesh = |nv: usize, ni: usize| DrawCmd::Mesh {
            origin: [0.0, 0.0],
            vertices: vec![MeshVertex { position: [0.0, 0.0], normal: [0.0, 0.0], color: [0.0; 4], stroke_px: 0.0, _pad: 0.0 }; nv],
            indices: vec![0u32; ni],
        };
        assert!(cmd_fits(&limits, &mesh(4, 6)), "small mesh fits");
        assert!(!cmd_fits(&limits, &mesh(100, 6)), "a vertex buffer over max_buffer_size declines");
        assert!(!cmd_fits(&limits, &mesh(4, 300)), "an index buffer over max_buffer_size declines");
        assert!(
            !cmd_fits(&limits, &DrawCmd::Image { pixmap: rustgis_engine::render::Pixmap::new(2049, 2).expect("pixmap") }),
            "an image over max_texture_dimension_2d declines"
        );
        // 0x0 and size-mismatched images are now unconstructible (Pixmap always holds w*h*4):
        // the type does the work.
        assert!(
            cmd_fits(&limits, &DrawCmd::Image { pixmap: rustgis_engine::render::Pixmap::new(2, 2).expect("pixmap") }),
            "a valid 2x2 image fits"
        );
    }

    #[test]
    fn gpu_renderer_constructs_and_uploads_headless() {
        let Some(rs) = headless_render_state() else { return };
        let r = GpuMapRenderer::new(&rs);
        // A tiny scene uploads: one triangle, one marker instance, one CPU image.
        let v =
            |x: f32, y: f32| MeshVertex { position: [x, y], normal: [0.0, 0.0], color: [1.0, 0.0, 0.0, 1.0], stroke_px: 0.0, _pad: 0.0 };
        let scene = GpuScene {
            clear: [0.0, 0.0, 0.0, 1.0],
            cmds: vec![
                DrawCmd::Mesh { origin: [0.0, 0.0], vertices: vec![v(0.0, 0.0), v(100.0, 0.0), v(0.0, 100.0)], indices: vec![0, 1, 2] },
                DrawCmd::Points {
                    origin: [0.0, 0.0],
                    instances: vec![PointInstance { position: [10.0, 10.0], color: [0.0, 1.0, 0.0, 1.0], radius_px: 4.0, shape: 0 }],
                },
                DrawCmd::Image { pixmap: rustgis_engine::render::Pixmap::new(1, 1).expect("pixmap") },
            ],
            labels: vec![],
            unplaced: vec![],
            attribution: vec![],
            tiles_pending: false,
            data_pending: false,
            tile_errors: vec![],
            tilt: None,
            clips: vec![],
        };
        let sb =
            SceneBuffers::from_scene(&rs.device, &rs.queue, &r, &scene, Camera::world(), [400.0, 300.0], 1.0).expect("fits the device");
        assert_eq!(sb.draws.len(), 3);
    }

    // ---- CPU↔GPU pixel parity: render the same map through both backends and compare ----

    use rustgis_engine::data::{Dataset, Geometry, GeometryType};
    use rustgis_engine::map::{Map, Marker, Renderer, Rgba, Symbol};
    use rustgis_engine::render::{self, Backend, RenderOutput, View};

    fn world_view() -> View {
        View { width: 640.0, height: 400.0, pixels_per_point: 1.0, camera: Camera::world(), sky: false }
    }

    /// Renders the GpuScene into an offscreen texture and reads it back (premultiplied RGBA,
    /// the same bytes the CPU pixmap holds). `None` on hosts without a GPU adapter.
    fn gpu_render_offscreen_on(map: &mut Map, view: &View, backends: wgpu::Backends) -> Option<Vec<u8>> {
        let rs = headless_render_state_on(backends)?;
        let r = GpuMapRenderer::new(&rs);
        let Some(RenderOutput::Gpu(scene)) = render::render_backend(map, view, Backend::Gpu) else { panic!("gpu-compatible scene") };
        let bufs = SceneBuffers::from_scene(
            &rs.device,
            &rs.queue,
            &r,
            &scene,
            view.camera,
            [view.width, view.height],
            view.pixels_per_point as f32,
        )?;
        let (w, h) = view.pixel_size();
        if w == 0 || h == 0 {
            return None;
        }
        // The production path: MSAA offscreen pass, resolved to a plain texture.
        let tgt = SceneTarget::new(&rs.device, &r, [w, h])?;
        let mut enc = rs.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("parity") });
        record_scene(&r, &bufs.draws, bufs.clear, &tgt, &mut enc);
        // Buffer copies need 256-aligned rows; pad and strip on readback.
        let row_bytes = (w * 4).div_ceil(256) * 256;
        let buf = rs.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("parity"),
            size: u64::from(row_bytes * h),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        enc.copy_texture_to_buffer(
            tgt.resolve_tex.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buf,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row_bytes), rows_per_image: Some(h) },
            },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        rs.queue.submit(Some(enc.finish()));
        let slice = buf.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |res| {
            let _ = tx.send(res);
        });
        rs.device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None }).ok()?;
        rx.recv().ok()?.ok()?;
        let data = slice.get_mapped_range().ok()?;
        let mut out = vec![0u8; (w * h * 4) as usize];
        for y in 0..h as usize {
            let src = y * row_bytes as usize;
            out[y * w as usize * 4..(y + 1) * w as usize * 4].copy_from_slice(data.get(src..src + w as usize * 4)?);
        }
        drop(data);
        buf.unmap();
        Some(out)
    }

    /// Pixels differing by >16 whose 3×3 neighbourhood in the CPU render is FLAT (max − min
    /// < 4 per channel). Edge pixels legitimately differ between analytic AA (CPU) and 4× MSAA
    /// (GPU) — worst on 1-2 px features at world zoom; a mismatch in a flat area is a
    /// structural bug (wrong colour, missing content, bad z-order).
    fn structural_outliers(cpu: &[u8], gpu: &[u8], w: usize, h: usize) -> usize {
        let mut out = 0;
        for y in 0..h {
            for x in 0..w {
                let i = (y * w + x) * 4;
                let worst = (0..4).map(|c| cpu[i + c].abs_diff(gpu[i + c])).max().unwrap_or(0);
                if worst <= 16 {
                    continue;
                }
                let mut lo = [255u8; 4];
                let mut hi = [0u8; 4];
                for dy in y.saturating_sub(1)..=(y + 1).min(h - 1) {
                    for dx in x.saturating_sub(1)..=(x + 1).min(w - 1) {
                        let j = (dy * w + dx) * 4;
                        for c in 0..4 {
                            lo[c] = lo[c].min(cpu[j + c]);
                            hi[c] = hi[c].max(cpu[j + c]);
                        }
                    }
                }
                if (0..4).all(|c| hi[c] - lo[c] < 4) {
                    out += 1;
                }
            }
        }
        out
    }

    /// Mean per-byte absolute difference and the fraction of pixels whose worst channel
    /// differs by more than 16.
    fn diff_stats(a: &[u8], b: &[u8]) -> (f64, f64) {
        assert_eq!(a.len(), b.len());
        let mut sum = 0u64;
        let mut over = 0usize;
        for (px, py) in a.chunks(4).zip(b.chunks(4)) {
            let mut worst = 0u8;
            for c in 0..4 {
                let d = px[c].abs_diff(py[c]);
                sum += u64::from(d);
                worst = worst.max(d);
            }
            if worst > 16 {
                over += 1;
            }
        }
        (sum as f64 / a.len() as f64, over as f64 / (a.len() / 4) as f64)
    }

    fn assert_parity(name: &str, build: impl Fn() -> Map, view: &View) {
        assert_parity_on(name, build, view, wgpu::Backends::all());
    }

    fn assert_parity_on(name: &str, build: impl Fn() -> Map, view: &View, backends: wgpu::Backends) {
        assert_parity_within(name, build, view, backends, 0);
    }

    /// [`assert_parity_on`], allowing `outliers` structural mismatches: a tilted view's CPU
    /// frame is warped from flat bands (resampled, so a thin line's colour spreads), its GPU one
    /// drawn in perspective directly.
    fn assert_parity_within(name: &str, build: impl Fn() -> Map, view: &View, backends: wgpu::Backends, outliers: usize) {
        let mut gm = build();
        let Some(gpu) = gpu_render_offscreen_on(&mut gm, view, backends) else { return }; // no adapter: skip
        let mut cm = build();
        let cpu = render::render(&mut cm, view).expect("cpu render");
        // PARITY_DUMP=/tmp/dir writes {name}-{cpu,gpu,diff}.png for eyeballing failures.
        if let Some(dir) = std::env::var_os("PARITY_DUMP") {
            let (w, h) = view.pixel_size();
            let dump = |suffix: &str, data: &[u8]| {
                let p = std::path::Path::new(&dir).join(format!("{}-{suffix}.png", name.replace(' ', "_")));
                let _ = image::save_buffer(&p, data, w, h, image::ColorType::Rgba8);
            };
            dump("cpu", cpu.pixmap.data());
            dump("gpu", &gpu);
            let mut diff: Vec<u8> = cpu.pixmap.data().iter().zip(&gpu).map(|(a, b)| a.abs_diff(*b).saturating_mul(8)).collect();
            for px in diff.chunks_mut(4) {
                px[3] = 255; // opaque, or the PNG previews as blank
            }
            dump("diff", &diff);
        }
        // Raw mean: same image overall. Structural: no mismatches away from edges — the CPU
        // anti-aliases analytically, the GPU via 4× MSAA, and on 1-2 px features their
        // coverage quantization legitimately differs (edge pixels are excluded by the
        // flatness test); wrong colours, missing content and bad z-order fail here.
        let (mean_raw, _) = diff_stats(cpu.pixmap.data(), &gpu);
        let (w, h) = view.pixel_size();
        let structural = structural_outliers(cpu.pixmap.data(), &gpu, w as usize, h as usize);
        assert!(mean_raw < 1.5 && structural <= outliers, "{name}: raw mean {mean_raw:.2}/255, {structural} structural outliers");
    }

    fn world_map() -> Map {
        let mut m = Map::new("Map");
        for (name, text) in [
            ("Countries", render::sample_data::COUNTRIES),
            ("Rivers", render::sample_data::RIVERS),
            ("Lakes", render::sample_data::LAKES),
            ("Cities", render::sample_data::CITIES),
        ] {
            if let Some(d) = render::world_dataset(name, text) {
                m.add_dataset(d);
            }
        }
        m
    }

    #[test]
    fn parity_world_sample() {
        assert_parity("world sample", world_map, &world_view());
    }

    /// The full camera matrix puts a point where `Camera::try_to_screen` does, turned and
    /// tilted, and its clip depth leaves out what is behind the eye.
    #[test]
    fn view_proj_matches_turned_and_tilted_cameras() {
        let (w, h) = (800.0, 500.0);
        let center = Coord::new(1_000_000.0, 6_000_000.0);
        let origin = [center.x + 1234.5, center.y - 777.0];
        for (bearing, pitch) in [(30.0, 0.0), (-120.0, 0.0), (0.0, 60.0), (45.0, 75.0), (170.0, 85.0)] {
            let cam = Camera { center, res: 300.0, bearing, pitch };
            let m = view_proj(&cam, w, h, origin);
            let clip = |p: Coord| -> [f64; 4] {
                let v = [(p.x - origin[0]) as f32, (p.y - origin[1]) as f32, 0.0, 1.0];
                std::array::from_fn(|r| f64::from((0..4).map(|c| m[c][r] * v[c]).sum::<f32>()))
            };
            for (dx, dy) in [(0.0, 0.0), (40_000.0, 30_000.0), (-60_000.0, 90_000.0), (20_000.0, -50_000.0)] {
                let p = Coord::new(center.x + dx, center.y + dy);
                let c = clip(p);
                match cam.try_to_screen(p, w, h) {
                    Some((sx, sy)) => {
                        assert!(c[2] >= 0.0 && c[2] <= c[3], "{bearing}/{pitch}: {c:?}");
                        let (gx, gy) = ((c[0] / c[3] + 1.0) / 2.0 * w, (1.0 - c[1] / c[3]) / 2.0 * h);
                        assert!((gx - sx).abs() < 0.05 && (gy - sy).abs() < 0.05, "{bearing}/{pitch}: ({gx}, {gy}) vs ({sx}, {sy})");
                    }
                    None => assert!(c[2] < 0.0, "{bearing}/{pitch}: behind the eye, clipped: {c:?}"),
                }
            }
        }
    }

    #[test]
    fn parity_turned_world_sample() {
        let view = View { camera: Camera { bearing: 35.0, ..Camera::world() }, ..world_view() };
        assert_parity("turned world sample", world_map, &view);
    }

    /// Structural mismatches a tilted view may have: 0.1 % of the 640 × 400 view, along thin
    /// lines and the coarse far bands near the horizon, which the CPU warps (resampling them)
    /// and the GPU draws in perspective directly.
    const TILTED_OUTLIERS: usize = 256;

    #[test]
    fn parity_tilted_world_sample() {
        let camera = Camera { center: Coord::new(1_100_000.0, 5_500_000.0), res: 6_000.0, bearing: 20.0, pitch: 60.0 };
        assert_parity_within("tilted world sample", world_map, &View { camera, ..world_view() }, wgpu::Backends::all(), TILTED_OUTLIERS);
    }

    #[test]
    fn parity_steep_world_sample_shows_the_sky() {
        // Past about 72° the horizon comes into view: the backdrop draws the sky above it and
        // fogs the water toward it, as the CPU warp does.
        let camera = Camera { center: Coord::new(1_100_000.0, 5_500_000.0), res: 6_000.0, bearing: -15.0, pitch: 80.0 };
        assert_parity_within("steep world sample", world_map, &View { camera, ..world_view() }, wgpu::Backends::all(), TILTED_OUTLIERS);
    }

    /// A tilted view's point layer stands its markers up once, over the bands: the bands draw
    /// none of them (as meshes or as a picture).
    #[test]
    fn a_tilted_view_draws_its_markers_once_upright() {
        let mut m = world_map();
        let camera = Camera { center: Coord::new(1_100_000.0, 5_500_000.0), res: 6_000.0, bearing: 20.0, pitch: 60.0 };
        let Some(RenderOutput::Gpu(scene)) = render::render_backend(&mut m, &View { camera, ..world_view() }, Backend::Gpu) else {
            panic!("a GPU scene")
        };
        let points: Vec<usize> =
            scene.cmds.iter().enumerate().filter(|(_, c)| matches!(c, DrawCmd::Points { .. })).map(|(i, _)| i).collect();
        assert_eq!(points.len(), 1, "one set of markers");
        assert!(points.iter().all(|i| scene.clips.get(*i).is_some_and(|c| c.upright)));
        assert!(!scene.cmds.iter().any(|c| matches!(c, DrawCmd::Image { .. })), "no pictures");
    }

    /// Blocks standing apart, lit, casting shadows, in a tilted view: the GPU's depth-tested
    /// roofs and walls match the CPU's far-to-near ones.
    fn blocks_map() -> Map {
        use rustgis_engine::data::{Dataset, Geometry, GeometryType};
        let mut m = Map::new("Map");
        let mut ds = Dataset::new("blocks", GeometryType::Polygon, vec![]);
        for (x, y) in [(9.0, 47.0), (11.5, 47.5), (10.0, 49.5), (12.5, 50.0), (8.0, 50.5)] {
            let ring =
                vec![Coord::new(x, y), Coord::new(x + 1.0, y), Coord::new(x + 1.0, y + 0.8), Coord::new(x, y + 0.8), Coord::new(x, y)];
            ds.push(Some(Geometry::Polygon(vec![vec![ring]])), vec![]);
        }
        let id = m.add_dataset(ds);
        if let Some(l) = m.layer_mut(id) {
            l.extrusion = Some(rustgis_engine::map::effects::Extrusion {
                field: None,
                height: 60_000.0,
                factor: 1.0,
                face_culling: true,
                lighting: true,
                shadows: true,
            });
        }
        m
    }

    #[test]
    fn parity_tilted_extrusions() {
        let camera =
            Camera { center: rustgis_engine::geom::lonlat_to_merc(Coord::new(10.5, 47.0)), res: 2_500.0, bearing: 25.0, pitch: 60.0 };
        let view = View { camera, ..world_view() };
        let mut m = blocks_map();
        let Some(RenderOutput::Gpu(scene)) = render::render_backend(&mut m, &view, Backend::Gpu) else { panic!("a GPU scene") };
        let solid =
            |shadows: bool| scene.cmds.iter().filter(|c| matches!(c, DrawCmd::Extrusion { shadows: s, .. } if *s == shadows)).count();
        assert_eq!((solid(false), solid(true)), (1, 1), "the blocks and their shadows");
        assert_parity_within("tilted extrusions", blocks_map, &view, wgpu::Backends::all(), TILTED_OUTLIERS);
    }

    /// A turned or tilted view that needs pictures drawn on the CPU (here a see-through layer,
    /// drawn apart and laid over) isn't a GPU scene: it comes back drawn on the CPU.
    #[test]
    fn a_tilted_view_needing_pictures_is_drawn_on_the_cpu() {
        let mut m = world_map();
        if let Some(l) = m.layers.iter_mut().find(|l| l.name == "Countries") {
            l.transparency = 50.0;
        }
        let camera = Camera { center: Coord::new(1_100_000.0, 5_500_000.0), res: 6_000.0, bearing: 20.0, pitch: 60.0 };
        assert!(matches!(render::render_backend(&mut m, &View { camera, ..world_view() }, Backend::Gpu), Some(RenderOutput::Cpu(_))));
        let mut m = world_map();
        assert!(matches!(render::render_backend(&mut m, &View { camera, ..world_view() }, Backend::Gpu), Some(RenderOutput::Gpu(_))));
    }

    #[test]
    fn parity_polar_projection() {
        // A bounded-area projection maps the far hemisphere to non-finite coordinates; both
        // backends break the affected rings there rather than bridging them (ring_path vs
        // decimate_runs). Regression: this failed with 1206 structural outliers when the GPU
        // bridged the gaps.
        assert_parity(
            "polar projection",
            || {
                let mut m = world_map();
                m.projection = rustgis_engine::map::Projection::Custom("northPolar".into());
                m
            },
            &View {
                width: 640.0,
                height: 400.0,
                pixels_per_point: 1.0,
                camera: Camera { center: Coord::new(0.0, 0.0), res: 15_000.0, bearing: 0.0, pitch: 0.0 },
                sky: false,
            },
        );
    }

    #[test]
    fn parity_world_sample_gl() {
        // The web build forces wgpu's GL backend (WebGL2): same scene, GL adapter.
        assert_parity_on("world sample (GL)", world_map, &world_view(), wgpu::Backends::GL);
    }

    fn world_map_midnight() -> Map {
        let mut m = world_map();
        m.basemap = rustgis_engine::map::Basemap::Midnight;
        m
    }

    #[test]
    fn parity_midnight_world_sample() {
        assert_parity("midnight world sample", world_map_midnight, &world_view());
    }

    #[test]
    fn parity_polygon_with_hole() {
        assert_parity(
            "polygon with hole",
            || {
                let mut m = Map::new("Map");
                let mut d = Dataset::new("holes", GeometryType::Polygon, vec![]);
                let ring = |x0: f64, y0: f64, x1: f64, y1: f64| {
                    vec![Coord::new(x0, y0), Coord::new(x1, y0), Coord::new(x1, y1), Coord::new(x0, y1)]
                };
                d.push(Some(Geometry::Polygon(vec![vec![ring(-60.0, -20.0, 0.0, 40.0), ring(-40.0, 0.0, -20.0, 20.0)]])), vec![]);
                m.add_dataset(d);
                m
            },
            &world_view(),
        );
    }

    #[test]
    fn parity_markers_all_shapes() {
        assert_parity(
            "all marker shapes",
            || {
                let mut m = Map::new("Map");
                for (i, marker) in Marker::ALL.into_iter().enumerate() {
                    let mut d = Dataset::new(format!("p{i}"), GeometryType::Point, vec![]);
                    d.push(Some(Geometry::Point(Coord::new(i as f64 * 25.0 - 50.0, 20.0))), vec![]);
                    let id = m.add_dataset(d);
                    if let Some(l) = m.layer_mut(id) {
                        l.renderer = Renderer::Single(Symbol {
                            fill: Rgba::rgb(0xE0, 0x7B, 0x39),
                            stroke: Rgba::rgb(255, 255, 255),
                            stroke_width: 1.5,
                            size: 12.0,
                            marker,
                        });
                    }
                }
                m
            },
            &world_view(),
        );
    }

    #[test]
    fn parity_lines_and_selection() {
        assert_parity(
            "lines and selection",
            || {
                let mut m = Map::new("Map");
                let mut d = Dataset::new("rivers", GeometryType::Polyline, vec![]);
                d.push(Some(Geometry::Line(vec![vec![Coord::new(-100.0, 0.0), Coord::new(-50.0, 20.0), Coord::new(0.0, 0.0)]])), vec![]);
                d.push(Some(Geometry::Line(vec![vec![Coord::new(0.0, -20.0), Coord::new(50.0, -10.0)]])), vec![]);
                let id = m.add_dataset(d);
                if let Some(l) = m.layer_mut(id) {
                    l.selection = vec![0];
                }
                m
            },
            &world_view(),
        );
    }

    #[test]
    fn parity_transparent_layer_interleaves() {
        assert_parity(
            "transparent layer interleave",
            || {
                let mut m = Map::new("Map");
                // Drawn bottom-up: last in the list first.
                let ring = |x0: f64| {
                    vec![vec![vec![Coord::new(x0, 0.0), Coord::new(x0 + 30.0, 0.0), Coord::new(x0 + 30.0, 30.0), Coord::new(x0, 30.0)]]]
                };
                let mut d = Dataset::new("a", GeometryType::Polygon, vec![]);
                d.push(Some(Geometry::Polygon(ring(-60.0))), vec![]);
                m.add_dataset(d);
                let mut d = Dataset::new("b", GeometryType::Polygon, vec![]);
                d.push(Some(Geometry::Polygon(ring(-20.0))), vec![]);
                let id = m.add_dataset(d);
                if let Some(l) = m.layer_mut(id) {
                    l.transparency = 50.0; // offscreen-composited → a CPU image command
                }
                let mut d = Dataset::new("c", GeometryType::Polygon, vec![]);
                d.push(Some(Geometry::Polygon(ring(20.0))), vec![]);
                m.add_dataset(d);
                m
            },
            &world_view(),
        );
    }
}
