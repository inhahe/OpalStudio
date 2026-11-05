//! Drawing the canvas on the GPU.
//!
//! The CPU composites the layers into the projection (see `opal_core::composite`); this
//! module shows projection tiles, at the pyramid level suited to the zoom, over a
//! checkerboard inside the canvas and the backdrop outside it, dimming content past the
//! canvas edge.
//!
//! Each tile is uploaded as a 258×258 texture: the tile with a one-pixel border copied from
//! its eight neighbors, so that filtering at a tile's edge sees the pixels beyond it and no
//! seams show. A texture is re-uploaded when its tile or any neighbor is replaced.

use eframe::egui_wgpu::{self, CallbackResources, CallbackTrait, ScreenDescriptor};
use eframe::wgpu;
use half::f16;
use opal_core::tile::{Pixel, TILE_SIZE, Tile, TileCoord};
use std::collections::HashMap;
use std::sync::Arc;

const TEX: u32 = TILE_SIZE + 2;
/// Frames a texture may go unused before it is freed.
const EVICT_AFTER: u64 = 600;

const SHADER: &str = r#"
struct View {
    vp_size: vec2<f32>,
    canvas_origin: vec2<f32>,
    canvas_size: vec2<f32>,
    checker_px: f32,
    off_canvas: f32,
    checker_a: vec4<f32>,
    checker_b: vec4<f32>,
    backdrop: vec4<f32>,
    encode_srgb: u32,
    filter_smooth: u32,
    _p1: u32,
    _p2: u32,
};

@group(0) @binding(0) var<uniform> view: View;
@group(1) @binding(0) var tile_tex: texture_2d<f32>;
@group(2) @binding(0) var tile_samp: sampler;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) px: vec2<f32>,
    @location(1) uv: vec2<f32>,
};

fn corner(i: u32) -> vec2<f32> {
    var c = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0),
        vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0), vec2<f32>(0.0, 1.0),
    );
    return c[i];
}

fn place(p: vec2<f32>) -> vec4<f32> {
    return vec4<f32>(p.x / view.vp_size.x * 2.0 - 1.0, 1.0 - p.y / view.vp_size.y * 2.0, 0.0, 1.0);
}

@vertex
fn vs_tile(@builtin(vertex_index) vi: u32, @location(0) rect: vec4<f32>) -> VsOut {
    let c = corner(vi);
    let p = mix(rect.xy, rect.zw, c);
    var out: VsOut;
    out.pos = place(p);
    out.px = p;
    out.uv = c;
    return out;
}

@vertex
fn vs_bg(@builtin(vertex_index) vi: u32) -> VsOut {
    let c = corner(vi);
    let p = c * view.vp_size;
    var out: VsOut;
    out.pos = place(p);
    out.px = p;
    out.uv = c;
    return out;
}

fn inside_canvas(px: vec2<f32>) -> bool {
    let l = px - view.canvas_origin;
    return all(l >= vec2<f32>(0.0)) && all(l < view.canvas_size);
}

fn background(px: vec2<f32>) -> vec3<f32> {
    if (!inside_canvas(px)) {
        return view.backdrop.rgb;
    }
    let cell = floor((px - view.canvas_origin) / view.checker_px);
    let odd = (i32(cell.x) + i32(cell.y)) & 1;
    if (odd == 0) {
        return view.checker_a.rgb;
    }
    return view.checker_b.rgb;
}

fn encode(c: vec3<f32>) -> vec4<f32> {
    if (view.encode_srgb == 0u) {
        return vec4<f32>(c, 1.0);
    }
    let lo = c * 12.92;
    let hi = 1.055 * pow(c, vec3<f32>(1.0 / 2.4)) - 0.055;
    return vec4<f32>(select(hi, lo, c <= vec3<f32>(0.0031308)), 1.0);
}

@fragment
fn fs_bg(in: VsOut) -> @location(0) vec4<f32> {
    return encode(background(in.px));
}

@fragment
fn fs_tile(in: VsOut) -> @location(0) vec4<f32> {
    let uv = (in.uv * TILE + 1.0) / TEX;
    var t: vec4<f32>;
    if (view.filter_smooth == 1u) {
        // Average four bilinear samples spread over the screen pixel's footprint. When the
        // level's pixels are smaller than screen pixels (up to half), plain bilinear
        // sampling skips some of them and fine detail turns into moiré; the four taps cover
        // the whole footprint.
        let dx = dpdx(uv) * 0.25;
        let dy = dpdy(uv) * 0.25;
        t = (textureSampleLevel(tile_tex, tile_samp, uv + dx + dy, 0.0)
            + textureSampleLevel(tile_tex, tile_samp, uv + dx - dy, 0.0)
            + textureSampleLevel(tile_tex, tile_samp, uv - dx + dy, 0.0)
            + textureSampleLevel(tile_tex, tile_samp, uv - dx - dy, 0.0)) * 0.25;
    } else {
        t = textureSampleLevel(tile_tex, tile_samp, uv, 0.0);
    }
    let a = clamp(t.a, 0.0, 1.0);
    var rgb = clamp(t.rgb, vec3<f32>(0.0), vec3<f32>(1.0e4)) + (1.0 - a) * background(in.px);
    if (!inside_canvas(in.px)) {
        rgb = mix(view.backdrop.rgb, rgb, view.off_canvas);
    }
    return encode(clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0)));
}
"#;

/// The shader, with the tile geometry it samples by (`TILE` interior pixels in `TEX`-wide
/// textures) taken from the constants the uploads use, so the two cannot disagree.
fn shader_source() -> String {
    format!("const TILE: f32 = {}.0;\nconst TEX: f32 = {}.0;\n{SHADER}", TILE_SIZE, TEX)
}

/// The view's parameters, laid out as the shader's `View` (96 bytes).
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ViewUniforms {
    /// Viewport size in device pixels.
    pub vp_size: [f32; 2],
    /// Where canvas pixel (0, 0) is, in device pixels from the viewport's corner.
    pub canvas_origin: [f32; 2],
    /// The canvas's size on screen, in device pixels.
    pub canvas_size: [f32; 2],
    /// Checkerboard square size in device pixels.
    pub checker_px: f32,
    /// How visible content past the canvas edge is, 0–1.
    pub off_canvas: f32,
    /// Linear-light colors.
    pub checker_a: [f32; 4],
    pub checker_b: [f32; 4],
    pub backdrop: [f32; 4],
    /// Set by the renderer: whether the shader must encode to sRGB itself.
    pub encode_srgb: u32,
    /// 1 to filter smoothly (averaging over each screen pixel), 0 for sharp pixels.
    pub smooth: u32,
    pub _pad: [u32; 2],
}

/// One tile to draw: where (device pixels, `[x0, y0, x1, y1]` from the viewport's corner)
/// and its pixels with its neighbors', row by row from the top-left (the tile itself is
/// `sources[4]`).
pub struct TileDraw {
    pub coord: TileCoord,
    pub rect: [f32; 4],
    pub sources: [Option<Arc<Tile>>; 9],
}

/// A frame's drawing of one canvas view.
pub struct CanvasCallback {
    /// Identifies the view (its document), for its buffers and texture cache.
    pub view_key: u64,
    pub level: u32,
    pub uniforms: ViewUniforms,
    pub tiles: Vec<TileDraw>,
    /// Sample pixels as sharp squares rather than smoothly.
    pub nearest: bool,
    /// How many tile textures may be kept, at most (those drawn this frame always are).
    pub texture_budget: usize,
}

/// The bytes a tile texture takes: 258 × 258 pixels of four half floats.
pub const TEXTURE_BYTES: u64 = (TEX as u64) * (TEX as u64) * 8;

type TileKey = (u64, u32, TileCoord);

struct GpuTile {
    texture: wgpu::Texture,
    bind_group: wgpu::BindGroup,
    sources: [Option<Arc<Tile>>; 9],
    last_used: u64,
}

struct ViewGpu {
    uniform: wgpu::Buffer,
    uniform_bg: wgpu::BindGroup,
    instances: wgpu::Buffer,
    capacity: usize,
    drawn: Vec<TileKey>,
    last_used: u64,
}

/// The renderer's GPU objects, kept in egui-wgpu's callback resources.
pub struct CanvasRenderer {
    tile_pipeline: wgpu::RenderPipeline,
    bg_pipeline: wgpu::RenderPipeline,
    view_bgl: wgpu::BindGroupLayout,
    tex_bgl: wgpu::BindGroupLayout,
    nearest: wgpu::BindGroup,
    linear: wgpu::BindGroup,
    encode_srgb: bool,
    tiles: HashMap<TileKey, GpuTile>,
    views: HashMap<u64, ViewGpu>,
    frame: u64,
}

impl CanvasRenderer {
    /// Creates the renderer for drawing into targets of `format`, and registers it.
    pub fn install(rs: &egui_wgpu::RenderState) {
        let r = CanvasRenderer::new(&rs.device, rs.target_format);
        rs.renderer.write().callback_resources.insert(r);
    }

    fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> CanvasRenderer {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("canvas"),
            source: wgpu::ShaderSource::Wgsl(shader_source().into()),
        });
        let view_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("canvas view"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let tex_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("canvas tile"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            }],
        });
        let samp_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("canvas sampler"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            }],
        });
        let sampler_group = |filter: wgpu::FilterMode| {
            let s = device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("canvas"),
                mag_filter: filter,
                min_filter: filter,
                address_mode_u: wgpu::AddressMode::ClampToEdge,
                address_mode_v: wgpu::AddressMode::ClampToEdge,
                ..Default::default()
            });
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("canvas sampler"),
                layout: &samp_bgl,
                entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::Sampler(&s) }],
            })
        };
        let nearest = sampler_group(wgpu::FilterMode::Nearest);
        let linear = sampler_group(wgpu::FilterMode::Linear);

        let target = [Some(wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::ALL })];
        let bg_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("canvas background"),
            bind_group_layouts: &[Some(&view_bgl)],
            immediate_size: 0,
        });
        let tile_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("canvas tiles"),
            bind_group_layouts: &[Some(&view_bgl), Some(&tex_bgl), Some(&samp_bgl)],
            immediate_size: 0,
        });
        let pipeline =
            |label, layout: &wgpu::PipelineLayout, vs, fs, buffers: &[Option<wgpu::VertexBufferLayout<'_>>]| {
                device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some(label),
                    layout: Some(layout),
                    vertex: wgpu::VertexState {
                        module: &module,
                        entry_point: Some(vs),
                        compilation_options: Default::default(),
                        buffers,
                    },
                    primitive: wgpu::PrimitiveState::default(),
                    depth_stencil: None,
                    multisample: wgpu::MultisampleState::default(),
                    fragment: Some(wgpu::FragmentState {
                        module: &module,
                        entry_point: Some(fs),
                        compilation_options: Default::default(),
                        targets: &target,
                    }),
                    multiview_mask: None,
                    cache: None,
                })
            };
        let instance_layout = wgpu::VertexBufferLayout {
            array_stride: 16,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &[wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x4,
                offset: 0,
                shader_location: 0,
            }],
        };
        let bg_pipeline = pipeline("canvas background", &bg_layout, "vs_bg", "fs_bg", &[]);
        let tile_pipeline = pipeline("canvas tiles", &tile_layout, "vs_tile", "fs_tile", &[Some(instance_layout)]);
        CanvasRenderer {
            tile_pipeline,
            bg_pipeline,
            view_bgl,
            tex_bgl,
            nearest,
            linear,
            encode_srgb: !format.is_srgb(),
            tiles: HashMap::new(),
            views: HashMap::new(),
            frame: 0,
        }
    }

    fn upload(&self, device: &wgpu::Device, queue: &wgpu::Queue, sources: &[Option<Arc<Tile>>; 9]) -> GpuTile {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("canvas tile"),
            size: wgpu::Extent3d { width: TEX, height: TEX, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        write_tile(queue, &texture, sources);
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("canvas tile"),
            layout: &self.tex_bgl,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) }],
        });
        GpuTile { texture, bind_group, sources: sources.clone(), last_used: self.frame }
    }

    /// How many tile textures are held (for diagnostics).
    pub fn texture_count(&self) -> usize {
        self.tiles.len()
    }
}

/// Writes the tile `sources[4]`, with its border, into `texture`.
fn write_tile(queue: &wgpu::Queue, texture: &wgpu::Texture, sources: &[Option<Arc<Tile>>; 9]) {
    let data = with_border(sources);
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        bytemuck::cast_slice(&data),
        wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(TEX * 8), rows_per_image: Some(TEX) },
        wgpu::Extent3d { width: TEX, height: TEX, depth_or_array_layers: 1 },
    );
}

/// The tile `sources[4]` with a one-pixel border from its neighbors (transparent where a
/// neighbor is absent), as `TEX × TEX` pixels.
fn with_border(sources: &[Option<Arc<Tile>>; 9]) -> Vec<Pixel> {
    let n = TILE_SIZE as usize;
    let w = TEX as usize;
    let mut out = vec![[f16::ZERO; 4]; w * w];
    // For each output pixel row/column, which neighbor (0, 1, 2 = before, same, after) and
    // which pixel of it supplies it.
    let pick = |o: usize| -> (usize, usize) {
        if o == 0 {
            (0, n - 1)
        } else if o == w - 1 {
            (2, 0)
        } else {
            (1, o - 1)
        }
    };
    for oy in 0..w {
        let (ny, sy) = pick(oy);
        // Interior rows copy a whole row of the middle tile at once.
        if let (1, Some(t)) = (ny, &sources[4]) {
            let row = &t.pixels()[sy * n..(sy + 1) * n];
            out[oy * w + 1..oy * w + 1 + n].copy_from_slice(row);
        }
        for ox in [0, w - 1].into_iter().chain(if ny == 1 { None } else { Some(1..w - 1) }.into_iter().flatten()) {
            let (nx, sx) = pick(ox);
            if let Some(t) = &sources[ny * 3 + nx] {
                out[oy * w + ox] = t.pixels()[sy * n + sx];
            }
        }
    }
    out
}

impl CallbackTrait for CanvasCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen: &ScreenDescriptor,
        _encoder: &mut wgpu::CommandEncoder,
        resources: &mut CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let Some(r) = resources.get_mut::<CanvasRenderer>() else { return Vec::new() };
        r.frame += 1;
        let frame = r.frame;
        let mut drawn = Vec::with_capacity(self.tiles.len());
        let mut instances: Vec<[f32; 4]> = Vec::with_capacity(self.tiles.len());
        for t in &self.tiles {
            let key = (self.view_key, self.level, t.coord);
            match r.tiles.get_mut(&key) {
                Some(g) => {
                    // A changed tile is rewritten into the texture it already has.
                    if !same_sources(&g.sources, &t.sources) {
                        write_tile(queue, &g.texture, &t.sources);
                        g.sources = t.sources.clone();
                    }
                    g.last_used = frame;
                }
                None => {
                    let g = r.upload(device, queue, &t.sources);
                    r.tiles.insert(key, g);
                }
            }
            drawn.push(key);
            instances.push(t.rect);
        }
        r.tiles.retain(|_, g| g.last_used + EVICT_AFTER > frame);
        for key in over_budget(r.tiles.iter().map(|(k, g)| (*k, g.last_used)), frame, self.texture_budget) {
            r.tiles.remove(&key);
        }

        let mut uniforms = self.uniforms;
        uniforms.encode_srgb = u32::from(r.encode_srgb);
        let needed = instances.len().max(1);
        let make_view = |capacity: usize| {
            let uniform = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("canvas view"),
                size: std::mem::size_of::<ViewUniforms>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let uniform_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("canvas view"),
                layout: &r.view_bgl,
                entries: &[wgpu::BindGroupEntry { binding: 0, resource: uniform.as_entire_binding() }],
            });
            let instances = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("canvas instances"),
                size: (capacity * 16) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            ViewGpu { uniform, uniform_bg, instances, capacity, drawn: Vec::new(), last_used: frame }
        };
        let too_small = r.views.get(&self.view_key).is_none_or(|v| v.capacity < needed);
        if too_small {
            let v = make_view(needed.next_power_of_two().max(64));
            r.views.insert(self.view_key, v);
        }
        let v = r.views.get_mut(&self.view_key).expect("just ensured");
        queue.write_buffer(&v.uniform, 0, bytemuck::bytes_of(&uniforms));
        if !instances.is_empty() {
            queue.write_buffer(&v.instances, 0, bytemuck::cast_slice(&instances));
        }
        v.drawn = drawn;
        v.last_used = frame;
        // A closed document's view is drawn no more; let its buffers go.
        r.views.retain(|_, v| v.last_used + EVICT_AFTER > frame);
        Vec::new()
    }

    fn paint(
        &self,
        _info: eframe::egui::PaintCallbackInfo,
        pass: &mut wgpu::RenderPass<'static>,
        resources: &CallbackResources,
    ) {
        let Some(r) = resources.get::<CanvasRenderer>() else { return };
        let Some(v) = r.views.get(&self.view_key) else { return };
        pass.set_pipeline(&r.bg_pipeline);
        pass.set_bind_group(0, &v.uniform_bg, &[]);
        pass.draw(0..6, 0..1);
        if v.drawn.is_empty() {
            return;
        }
        pass.set_pipeline(&r.tile_pipeline);
        pass.set_bind_group(0, &v.uniform_bg, &[]);
        pass.set_bind_group(2, if self.nearest { &r.nearest } else { &r.linear }, &[]);
        pass.set_vertex_buffer(0, v.instances.slice(..));
        for (i, key) in v.drawn.iter().enumerate() {
            if let Some(g) = r.tiles.get(key) {
                pass.set_bind_group(1, &g.bind_group, &[]);
                pass.draw(0..6, i as u32..i as u32 + 1);
            }
        }
    }
}

/// Which of `tiles` (each with the frame it was last drawn) to free so that at most
/// `budget` remain, least recently drawn first, never one drawn in `frame`.
fn over_budget(tiles: impl Iterator<Item = (TileKey, u64)>, frame: u64, budget: usize) -> Vec<TileKey> {
    let tiles: Vec<(TileKey, u64)> = tiles.collect();
    let excess = tiles.len().saturating_sub(budget);
    let mut old: Vec<(u64, TileKey)> = tiles.into_iter().filter(|&(_, f)| f != frame).map(|(k, f)| (f, k)).collect();
    old.sort_unstable_by_key(|&(f, _)| f);
    old.into_iter().take(excess).map(|(_, k)| k).collect()
}

fn same_sources(a: &[Option<Arc<Tile>>; 9], b: &[Option<Arc<Tile>>; 9]) -> bool {
    a.iter().zip(b).all(|(x, y)| match (x, y) {
        (None, None) => true,
        (Some(x), Some(y)) => Arc::ptr_eq(x, y),
        _ => false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use opal_core::tile::pixel_from_f32;

    #[test]
    fn textures_past_the_budget_go_least_recently_drawn_first() {
        let key = |x| (1u64, 0u32, TileCoord::new(x, 0));
        let tiles = [(key(0), 5), (key(1), 9), (key(2), 2), (key(3), 9), (key(4), 7)];
        // Five held, three allowed: the two drawn longest ago go.
        let mut gone = over_budget(tiles.into_iter(), 9, 3);
        gone.sort_unstable();
        assert_eq!(gone, vec![key(0), key(2)]);
        // Never one drawn this frame, even past the budget.
        assert_eq!(over_budget(tiles.into_iter(), 9, 0).len(), 3);
        assert!(over_budget(tiles.into_iter(), 9, 5).is_empty());
    }

    #[test]
    fn the_border_comes_from_the_neighbors() {
        let mut center = Tile::transparent();
        center.set(0, 0, pixel_from_f32([1.0, 0.0, 0.0, 1.0]));
        let mut right = Tile::transparent();
        right.set(0, 7, pixel_from_f32([0.0, 1.0, 0.0, 1.0]));
        let mut below_left = Tile::transparent();
        below_left.set(255, 0, pixel_from_f32([0.0, 0.0, 1.0, 1.0]));
        let mut sources: [Option<Arc<Tile>>; 9] = Default::default();
        sources[4] = Some(Arc::new(center));
        sources[5] = Some(Arc::new(right));
        sources[6] = Some(Arc::new(below_left));
        let out = with_border(&sources);
        let w = TEX as usize;
        assert_eq!(out[w + 1], pixel_from_f32([1.0, 0.0, 0.0, 1.0]), "interior is offset by one");
        assert_eq!(out[8 * w + (w - 1)], pixel_from_f32([0.0, 1.0, 0.0, 1.0]), "right border, row 7");
        assert_eq!(out[(w - 1) * w], pixel_from_f32([0.0, 0.0, 1.0, 1.0]), "bottom-left corner");
        assert_eq!(out[0], [f16::ZERO; 4], "no top-left neighbor");
    }
}
