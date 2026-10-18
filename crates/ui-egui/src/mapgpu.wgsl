//! GPU map shaders: polygon/line meshes with screen-constant stroke width, instanced marker
//! shapes, and full-view CPU-image compositing. Premultiplied alpha throughout (the CPU
//! renderer and egui agree on that). All textures and the egui target are linear
//! (`Rgba8Unorm`): values pass through with no conversion, matching the CPU path byte for byte.

struct CmdUniforms {
    view_proj: mat4x4f,
    /// The depth row of the camera the scene was drawn for (a tilted scene's bands).
    scene_depth: vec4f,
    /// The fog a tilted view's ground fades into (premultiplied).
    fog: vec4f,
    /// Map units per physical pixel (scales stroke normals), 1 for upright markers, the band's
    /// nearest and farthest depths.
    params: vec4f,
    /// Physical pixel size of the map view (scales marker pixel offsets), 1 when tilted, and
    /// the reach's depth.
    viewport: vec4f,
    /// Extrusions: the light's direction (map units) and how far a shadow falls per map unit
    /// of height.
    light: vec4f,
};

struct VOut {
    @builtin(position) clip: vec4f,
    @location(0) color: vec4f,
    /// The depth now, and for the camera the scene was drawn for.
    @location(1) depth: vec2f,
};

/// A tilted view's ground: a band keeps to its own stretch of it, nothing shows past the
/// reach (the sky does), and it fades into fog from halfway there, as the CPU path warps it.
fn tilted(c: vec4f, depth: vec2f, u: CmdUniforms) -> vec4f {
    let reach = u.viewport.w;
    let t = smoothstep(reach * 0.5, reach, depth.x);
    return vec4f(mix(c.rgb, u.fog.rgb * c.a, t), c.a);
}

fn kept(depth: vec2f, u: CmdUniforms) -> bool {
    return u.viewport.z < 0.5 || (depth.y >= u.params.z && depth.y < u.params.w && depth.x <= u.viewport.w);
}

// ---- mesh (polygon fills and line/ring strokes) ----

struct MeshIn {
    @location(0) position: vec2f,
    @location(1) normal: vec2f,
    @location(2) color: vec4f,
    @location(3) stroke_px: f32,
};

@group(0) @binding(0) var<uniform> u_mesh: CmdUniforms;

@vertex
fn vs_mesh(in: MeshIn) -> VOut {
    // Fill vertices have normal = (0,0) and stroke_px = 0: no displacement. A stroke's width is
    // pixels, so in perspective it scales by the depth (1 in a flat view), as marker offsets do,
    // keeping its width on screen as the CPU path's bands about do.
    let depth = (u_mesh.view_proj * vec4f(in.position, 0.0, 1.0)).w;
    let world = vec4f(in.position + in.normal * (in.stroke_px * u_mesh.params.x * 0.5 * max(depth, 0.0)), 0.0, 1.0);
    var out: VOut;
    out.clip = u_mesh.view_proj * world;
    out.color = in.color;
    out.depth = vec2f(out.clip.w, dot(u_mesh.scene_depth, world));
    return out;
}

@fragment
fn fs_mesh(in: VOut) -> @location(0) vec4f {
    if !kept(in.depth, u_mesh) {
        discard;
    }
    if u_mesh.viewport.z > 0.5 {
        return tilted(in.color, in.depth, u_mesh);
    }
    return in.color;
}

// ---- markers (instanced unit-space shape templates) ----

// Five shapes padded to 72 vertices each (the circle's 24 fan triangles). Padding points sit
// at the shape centre: zero-area triangles, discarded by the rasteriser.
struct TemplateBank {
    verts: array<vec4f, 360>,
};

@group(0) @binding(0) var<uniform> u_mark: CmdUniforms;
@group(0) @binding(1) var<uniform> u_tpl: TemplateBank;

struct MarkIn {
    @builtin(vertex_index) vi: u32,
    @location(0) inst_pos: vec2f,
    @location(1) inst_color: vec4f,
    @location(2) inst_radius: f32,
    @location(3) inst_shape: u32,
};

@vertex
fn vs_marker(in: MarkIn) -> VOut {
    let tpl = u_tpl.verts[in.inst_shape * 72u + in.vi].xy;
    let at = vec4f(in.inst_pos, 0.0, 1.0);
    let center = u_mark.view_proj * at;
    // An upright marker grows nearer the eye and shrinks farther away (as
    // `render::UprightMarker::place` sizes it); any other keeps its size on screen.
    var k = 1.0;
    if u_mark.params.y > 0.5 {
        k = clamp(1.0 / max(center.w, 1e-6), 0.25, 2.0);
    }
    // Templates are y-down (screen), clip is y-up; pixel offsets scale by w to stay pixels.
    let offset = vec2f(tpl.x, -tpl.y) * in.inst_radius * 2.0 / u_mark.viewport.xy * k * center.w;
    var out: VOut;
    out.clip = center + vec4f(offset, 0.0, 0.0);
    out.color = in.inst_color;
    // The whole marker at its centre's depth, so a band keeps or leaves it whole.
    out.depth = vec2f(center.w, dot(u_mark.scene_depth, at));
    return out;
}

@fragment
fn fs_marker(in: VOut) -> @location(0) vec4f {
    if !kept(in.depth, u_mark) {
        discard;
    }
    if u_mark.viewport.z > 0.5 {
        return tilted(in.color, in.depth, u_mark);
    }
    return in.color;
}

// ---- extrusions (roofs and walls standing up in a tilted view, and their shadows) ----

struct ExtrudeIn {
    @location(0) position: vec3f,
    @location(1) normal: vec2f,
    @location(2) color: vec4f,
    /// Up the wall (0 foot, 1 top), the part (0 roof, 1 wall, 2 shadow), 1 when lit, and how
    /// far a shadow's corner falls (its feature's height).
    @location(3) info: vec4f,
};

@group(0) @binding(0) var<uniform> u_ext: CmdUniforms;

@vertex
fn vs_extrude(in: ExtrudeIn) -> VOut {
    var p = in.position;
    if in.info.y > 1.5 {
        // A shadow on the ground, away from the light.
        p = vec3f(p.xy + u_ext.light.zw * in.info.w, 0.0);
    }
    let world = vec4f(p, 1.0);
    var out: VOut;
    out.clip = u_ext.view_proj * world;
    var c = in.color;
    if in.info.y > 0.5 && in.info.y < 1.5 {
        // A wall, lit by the way it faces and darker toward its foot (as `render::tilted_walls`).
        var shade = 0.8;
        if in.info.z > 0.5 {
            let lit = max(dot(in.normal, u_ext.light.xy), 0.0);
            shade = (0.6 + 0.4 * lit) * mix(0.72, 1.0, in.info.x);
        }
        c = vec4f(c.rgb * shade, c.a);
    }
    out.color = c;
    out.depth = vec2f(out.clip.w, dot(u_ext.scene_depth, world));
    return out;
}

@fragment
fn fs_extrude(in: VOut) -> @location(0) vec4f {
    if !kept(in.depth, u_ext) {
        discard;
    }
    if u_ext.viewport.z > 0.5 {
        return tilted(in.color, in.depth, u_ext);
    }
    return in.color;
}

// ---- backdrop (a tilted view: water fading into fog toward the reach, the sky above) ----

struct Backdrop {
    /// The view's height (logical px), the eye's distance from the centre, the pitch's cosine
    /// and sine.
    tilt: vec4f,
    /// Physical pixels per point, the reach's screen row (logical px), the reach's depth.
    params: vec4f,
    water: vec4f,
    fog: vec4f,
    sky: vec4f,
};

@group(0) @binding(0) var<uniform> u_back: Backdrop;

@vertex
fn vs_backdrop(@builtin(vertex_index) vi: u32) -> @builtin(position) vec4f {
    var pos = array<vec2f, 6>(
        vec2f(-1.0, -1.0), vec2f(1.0, -1.0), vec2f(1.0, 1.0),
        vec2f(-1.0, -1.0), vec2f(1.0, 1.0), vec2f(-1.0, 1.0),
    );
    return vec4f(pos[vi], 0.0, 1.0);
}

@fragment
fn fs_backdrop(@builtin(position) pos: vec4f) -> @location(0) vec4f {
    // As `Camera::ground_lookup` finds the ground under a screen row.
    let y = pos.y / u_back.params.x;
    let h = u_back.tilt.x;
    let d = u_back.tilt.y;
    let reach = u_back.params.z;
    let b = y - h * 0.5;
    let den = d * u_back.tilt.z + b * u_back.tilt.w;
    if den > 0.0 {
        let v = b * d / den;
        let depth = (d - v * u_back.tilt.w) / d;
        if depth <= reach {
            return mix(u_back.water, u_back.fog, smoothstep(reach * 0.5, reach, depth));
        }
    }
    return mix(u_back.sky, u_back.fog, clamp(y / u_back.params.y, 0.0, 1.0));
}

// ---- image (a CPU-rasterized run, composited covering the whole map view) ----

struct ImgOut {
    @builtin(position) clip: vec4f,
    @location(0) uv: vec2f,
};

@vertex
fn vs_image(@builtin(vertex_index) vi: u32) -> ImgOut {
    var pos = array<vec2f, 6>(
        vec2f(-1.0, -1.0), vec2f(1.0, -1.0), vec2f(1.0, 1.0),
        vec2f(-1.0, -1.0), vec2f(1.0, 1.0), vec2f(-1.0, 1.0),
    );
    let p = pos[vi];
    var out: ImgOut;
    out.clip = vec4f(p, 0.0, 1.0);
    // Pixmap row 0 is the top: clip y=+1 maps to texture v=0.
    out.uv = vec2f(p.x * 0.5 + 0.5, 0.5 - p.y * 0.5);
    return out;
}

@group(0) @binding(0) var t_img: texture_2d<f32>;
@group(0) @binding(1) var s_img: sampler;

@fragment
fn fs_image(in: ImgOut) -> @location(0) vec4f {
    return textureSample(t_img, s_img, in.uv);
}
