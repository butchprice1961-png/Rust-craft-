//! GPU rendering for RustCraft's 1.8 engine (wgpu: Vulkan, Metal, DirectX
//! 12 or OpenGL).
//!
//! Chunk sections are meshed on worker threads ([`mesh`]) and uploaded as
//! one vertex/index buffer pair per section and render layer. Each frame
//! culls sections against the view frustum and render distance, draws
//! opaque and cut-out geometry front to back, then translucent geometry
//! (water, ice, stained glass) back to front.
//!
//! Known gaps: translucent faces are sorted per section, not per face;
//! there is no occlusion culling beyond the frustum yet.

pub mod camera;
pub mod mesh;
pub mod textures;

use std::collections::HashMap;

use glam::{DVec3, IVec3, Vec3};
use wgpu::util::DeviceExt;

pub use camera::{Camera, Frustum};
pub use mesh::{MeshData, Snapshot, Vertex};
pub use textures::{BiomeColors, Lookup, Source, TextureData};

use rustcraft_world::block::Aabb;

pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Globals {
    view_proj: [[f32; 4]; 4],
    fog_color: [f32; 4],
    params: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct LineVertex {
    /// Camera-relative position.
    pub pos: [f32; 3],
    pub color: [f32; 4],
}

struct GpuMesh {
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    count: u32,
}

#[derive(Default)]
struct GpuSection {
    solid: Option<GpuMesh>,
    translucent: Option<GpuMesh>,
}

/// What the world looks like this frame.
pub struct Scene<'a> {
    pub camera: Camera,
    /// Sky colour (also the fog colour), linear 0–1.
    pub sky_color: [f32; 3],
    /// Multiplier for sky light: 1 at noon, about 0.2 at night.
    pub sky_brightness: f32,
    /// In chunks.
    pub render_distance: u32,
    /// Targeted block outline: block position and its selection boxes.
    pub selection: Option<(IVec3, &'a [Aabb])>,
    /// Extra camera-relative line segments (debug views).
    pub lines: &'a [LineVertex],
}

#[derive(Default, Debug, Clone, Copy)]
pub struct Stats {
    pub sections_loaded: usize,
    pub sections_drawn: usize,
    pub triangles: usize,
    pub gpu_bytes: u64,
}

pub struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    format: wgpu::TextureFormat,
    depth: wgpu::TextureView,
    size: (u32, u32),
    globals: wgpu::Buffer,
    offsets: wgpu::Buffer,
    offsets_capacity: u64,
    layout: wgpu::BindGroupLayout,
    bind_group: wgpu::BindGroup,
    solid: wgpu::RenderPipeline,
    translucent: wgpu::RenderPipeline,
    lines: wgpu::RenderPipeline,
    line_buffer: wgpu::Buffer,
    line_capacity: u64,
    atlas: wgpu::Texture,
    atlas_view: wgpu::TextureView,
    sampler: wgpu::Sampler,
    tile: u32,
    mip_levels: u32,
    animations: Vec<textures::Animation>,
    shown_frames: Vec<usize>,
    sections: HashMap<IVec3, GpuSection>,
    pub lookup: Lookup,
    pub colors: BiomeColors,
    pub stats: Stats,
}

fn mip_chain(rgba: &[u8], size: u32, levels: u32) -> Vec<Vec<u8>> {
    let mut out = vec![rgba.to_vec()];
    let mut s = size;
    for _ in 1..levels {
        let prev = out.last().unwrap();
        let n = (s / 2).max(1);
        let mut next = vec![0u8; (n * n * 4) as usize];
        for y in 0..n {
            for x in 0..n {
                // Alpha-weighted 2×2 average so transparent texels do not darken edges.
                let (mut c, mut a) = ([0f32; 3], 0f32);
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let i = (((y * 2 + dy).min(s - 1) * s + (x * 2 + dx).min(s - 1)) * 4) as usize;
                    let alpha = prev[i + 3] as f32;
                    for k in 0..3 {
                        c[k] += prev[i + k] as f32 * alpha;
                    }
                    a += alpha;
                }
                let o = ((y * n + x) * 4) as usize;
                for k in 0..3 {
                    next[o + k] = if a > 0.0 { (c[k] / a) as u8 } else { 0 };
                }
                next[o + 3] = (a / 4.0) as u8;
            }
        }
        out.push(next);
        s = n;
    }
    out
}

impl Renderer {
    pub fn new(
        device: wgpu::Device,
        queue: wgpu::Queue,
        format: wgpu::TextureFormat,
        width: u32,
        height: u32,
        textures: &TextureData,
    ) -> Self {
        let tile = textures.size;
        let mip_levels = (tile.ilog2() + 1).min(5);
        let layers = (textures.layers.len() as u32).min(device.limits().max_texture_array_layers);
        if layers < textures.layers.len() as u32 {
            tracing::warn!("GPU supports only {layers} texture layers; some textures will be missing");
        }
        let atlas = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("block textures"),
            size: wgpu::Extent3d { width: tile, height: tile, depth_or_array_layers: layers },
            mip_level_count: mip_levels,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let atlas_view = atlas.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("blocks"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            ..Default::default()
        });

        let globals = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("globals"),
            size: std::mem::size_of::<Globals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let offsets_capacity = 4096;
        let offsets = Self::offsets_buffer(&device, offsets_capacity);
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("world"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let bind_group = Self::bind_group(&device, &layout, &globals, &offsets, &atlas_view, &sampler);

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("world shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shader.wgsl").into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("world"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let vertex_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Vertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x2, 2 => Uint32, 3 => Unorm8x4, 4 => Unorm8x4],
        };
        let line_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<LineVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x4],
        };
        let pipeline = |label: &str,
                        layout: wgpu::VertexBufferLayout<'_>,
                        vs: &str,
                        fs: &str,
                        topology: wgpu::PrimitiveTopology,
                        blend: Option<wgpu::BlendState>,
                        depth_write: bool,
                        cull: Option<wgpu::Face>| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some(vs),
                    compilation_options: Default::default(),
                    buffers: &[Some(layout)],
                },
                primitive: wgpu::PrimitiveState { topology, cull_mode: cull, ..Default::default() },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: Some(depth_write),
                    depth_compare: Some(wgpu::CompareFunction::LessEqual),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(fs),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState { format, blend, write_mask: wgpu::ColorWrites::ALL })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let tri = wgpu::PrimitiveTopology::TriangleList;
        let solid =
            pipeline("solid", vertex_layout.clone(), "vs_main", "fs_solid", tri, None, true, Some(wgpu::Face::Back));
        let translucent = pipeline(
            "translucent",
            vertex_layout,
            "vs_main",
            "fs_translucent",
            tri,
            Some(wgpu::BlendState::ALPHA_BLENDING),
            false,
            Some(wgpu::Face::Back),
        );
        let lines = pipeline(
            "lines",
            line_layout,
            "vs_line",
            "fs_line",
            wgpu::PrimitiveTopology::LineList,
            Some(wgpu::BlendState::ALPHA_BLENDING),
            false,
            None,
        );
        let line_capacity = 1024;
        let line_buffer = Self::line_buffer(&device, line_capacity);

        let mut r = Renderer {
            depth: Self::depth_view(&device, width, height),
            size: (width.max(1), height.max(1)),
            globals,
            offsets,
            offsets_capacity,
            layout,
            bind_group,
            solid,
            translucent,
            lines,
            line_buffer,
            line_capacity,
            atlas,
            atlas_view,
            sampler,
            tile,
            mip_levels,
            animations: Vec::new(),
            shown_frames: Vec::new(),
            sections: HashMap::new(),
            lookup: textures.lookup.clone(),
            colors: textures.biome_colors(),
            stats: Stats::default(),
            device,
            queue,
            format,
        };
        for (i, layer) in textures.layers.iter().enumerate().take(layers as usize) {
            r.write_layer(i as u32, layer);
        }
        r.animations = textures
            .animations
            .iter()
            .filter(|a| a.layer < layers)
            .map(|a| textures::Animation { layer: a.layer, frames: a.frames.clone(), sequence: a.sequence.clone() })
            .collect();
        r.shown_frames = vec![0; r.animations.len()];
        r
    }

    fn offsets_buffer(device: &wgpu::Device, capacity: u64) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("section offsets"),
            size: capacity * 16,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    fn line_buffer(device: &wgpu::Device, capacity: u64) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("lines"),
            size: capacity * std::mem::size_of::<LineVertex>() as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    fn bind_group(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        globals: &wgpu::Buffer,
        offsets: &wgpu::Buffer,
        atlas: &wgpu::TextureView,
        sampler: &wgpu::Sampler,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("world"),
            layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: globals.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: offsets.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(atlas) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Sampler(sampler) },
            ],
        })
    }

    fn depth_view(device: &wgpu::Device, width: u32, height: u32) -> wgpu::TextureView {
        device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("depth"),
                size: wgpu::Extent3d { width: width.max(1), height: height.max(1), depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: DEPTH_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            })
            .create_view(&Default::default())
    }

    fn write_layer(&self, layer: u32, rgba: &[u8]) {
        for (level, data) in mip_chain(rgba, self.tile, self.mip_levels).iter().enumerate() {
            let s = (self.tile >> level).max(1);
            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.atlas,
                    mip_level: level as u32,
                    origin: wgpu::Origin3d { x: 0, y: 0, z: layer },
                    aspect: wgpu::TextureAspect::All,
                },
                data,
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(s * 4), rows_per_image: Some(s) },
                wgpu::Extent3d { width: s, height: s, depth_or_array_layers: 1 },
            );
        }
    }

    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }

    pub fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }

    pub fn format(&self) -> wgpu::TextureFormat {
        self.format
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if (width, height) != self.size && width > 0 && height > 0 {
            self.size = (width, height);
            self.depth = Self::depth_view(&self.device, width, height);
        }
    }

    /// Advances animated textures (water, lava, fire, portals) to `tick`.
    pub fn animate(&mut self, tick: u64) {
        for i in 0..self.animations.len() {
            let frame = self.animations[i].frame_at(tick);
            if frame != self.shown_frames[i] {
                self.shown_frames[i] = frame;
                let a = &self.animations[i];
                self.write_layer(a.layer, &a.frames[frame]);
            }
        }
    }

    fn upload_geometry(&self, g: &mesh::Geometry) -> Option<GpuMesh> {
        if g.is_empty() {
            return None;
        }
        Some(GpuMesh {
            vertices: self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("section vertices"),
                contents: bytemuck::cast_slice(&g.vertices),
                usage: wgpu::BufferUsages::VERTEX,
            }),
            indices: self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("section indices"),
                contents: bytemuck::cast_slice(&g.indices),
                usage: wgpu::BufferUsages::INDEX,
            }),
            count: g.indices.len() as u32,
        })
    }

    /// Replaces a section's meshes. Empty meshes remove the section.
    pub fn upload(&mut self, mesh: &MeshData) {
        let section = GpuSection {
            solid: self.upload_geometry(&mesh.solid),
            translucent: self.upload_geometry(&mesh.translucent),
        };
        if section.solid.is_none() && section.translucent.is_none() {
            self.sections.remove(&mesh.section);
        } else {
            self.sections.insert(mesh.section, section);
        }
    }

    pub fn remove_section(&mut self, section: IVec3) {
        self.sections.remove(&section);
    }

    pub fn remove_column(&mut self, cx: i32, cz: i32) {
        self.sections.retain(|s, _| s.x != cx || s.z != cz);
    }

    pub fn clear(&mut self) {
        self.sections.clear();
    }

    pub fn has_section(&self, section: IVec3) -> bool {
        self.sections.contains_key(&section)
    }

    /// Draws the world into `target`.
    pub fn render(&mut self, encoder: &mut wgpu::CommandEncoder, target: &wgpu::TextureView, scene: &Scene<'_>) {
        let cam = scene.camera;
        let view_proj = cam.view_proj();
        let frustum = Frustum::from_matrix(view_proj);
        let (ccx, ccz) = ((cam.pos.x.floor() as i32) >> 4, (cam.pos.z.floor() as i32) >> 4);
        let rd = scene.render_distance as i32;

        let mut visible: Vec<(IVec3, Vec3, f32)> = self
            .sections
            .keys()
            .filter(|s| (s.x - ccx).abs() <= rd && (s.z - ccz).abs() <= rd)
            .filter_map(|s| {
                let rel = ((*s * 16).as_dvec3() - cam.pos).as_vec3();
                frustum.contains_box(rel, rel + Vec3::splat(16.0)).then(|| (*s, rel, (rel + 8.0).length_squared()))
            })
            .collect();
        visible.sort_by(|a, b| a.2.total_cmp(&b.2));

        if visible.len() as u64 > self.offsets_capacity {
            self.offsets_capacity = (visible.len() as u64).next_power_of_two();
            self.offsets = Self::offsets_buffer(&self.device, self.offsets_capacity);
            self.bind_group = Self::bind_group(
                &self.device,
                &self.layout,
                &self.globals,
                &self.offsets,
                &self.atlas_view,
                &self.sampler,
            );
        }
        let offsets: Vec<[f32; 4]> = visible.iter().map(|(_, rel, _)| rel.extend(0.0).to_array()).collect();
        if !offsets.is_empty() {
            self.queue.write_buffer(&self.offsets, 0, bytemuck::cast_slice(&offsets));
        }
        let far = (scene.render_distance.max(2) * 16) as f32;
        let gamma = if self.format.is_srgb() { 2.2 } else { 1.0 };
        let globals = Globals {
            view_proj: view_proj.to_cols_array_2d(),
            fog_color: [scene.sky_color[0], scene.sky_color[1], scene.sky_color[2], 1.0],
            params: [far * 0.6, far * 0.95, scene.sky_brightness, gamma],
        };
        self.queue.write_buffer(&self.globals, 0, bytemuck::bytes_of(&globals));

        let mut line_vertices: Vec<LineVertex> = scene.lines.to_vec();
        if let Some((pos, boxes)) = scene.selection {
            let base = (pos.as_dvec3() - cam.pos).as_vec3();
            for b in boxes {
                push_box(&mut line_vertices, base + b.min - 0.002, base + b.max + 0.002, [0.0, 0.0, 0.0, 0.45]);
            }
        }
        if line_vertices.len() as u64 > self.line_capacity {
            self.line_capacity = (line_vertices.len() as u64).next_power_of_two();
            self.line_buffer = Self::line_buffer(&self.device, self.line_capacity);
        }
        if !line_vertices.is_empty() {
            self.queue.write_buffer(&self.line_buffer, 0, bytemuck::cast_slice(&line_vertices));
        }

        let sky = scene.sky_color.map(|c| if self.format.is_srgb() { c.powf(2.2) } else { c } as f64);
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("world"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color { r: sky[0], g: sky[1], b: sky[2], a: 1.0 }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &self.depth,
                depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_bind_group(0, &self.bind_group, &[]);
        let mut triangles = 0;
        pass.set_pipeline(&self.solid);
        for (i, (s, _, _)) in visible.iter().enumerate() {
            if let Some(m) = self.sections.get(s).and_then(|g| g.solid.as_ref()) {
                pass.set_vertex_buffer(0, m.vertices.slice(..));
                pass.set_index_buffer(m.indices.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..m.count, 0, i as u32..i as u32 + 1);
                triangles += m.count as usize / 3;
            }
        }
        pass.set_pipeline(&self.translucent);
        for (i, (s, _, _)) in visible.iter().enumerate().rev() {
            if let Some(m) = self.sections.get(s).and_then(|g| g.translucent.as_ref()) {
                pass.set_vertex_buffer(0, m.vertices.slice(..));
                pass.set_index_buffer(m.indices.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..m.count, 0, i as u32..i as u32 + 1);
                triangles += m.count as usize / 3;
            }
        }
        if !line_vertices.is_empty() {
            pass.set_pipeline(&self.lines);
            pass.set_vertex_buffer(0, self.line_buffer.slice(..));
            pass.draw(0..line_vertices.len() as u32, 0..1);
        }
        drop(pass);
        self.stats = Stats {
            sections_loaded: self.sections.len(),
            sections_drawn: visible.len(),
            triangles,
            gpu_bytes: self
                .sections
                .values()
                .flat_map(|s| [&s.solid, &s.translucent])
                .flatten()
                .map(|m| m.vertices.size() + m.indices.size())
                .sum(),
        };
    }
}

/// The twelve edges of a box as line-list vertices.
pub fn push_box(out: &mut Vec<LineVertex>, min: Vec3, max: Vec3, color: [f32; 4]) {
    let c = |x: bool, y: bool, z: bool| {
        Vec3::new(if x { max.x } else { min.x }, if y { max.y } else { min.y }, if z { max.z } else { min.z })
    };
    for (a, b) in [
        ((false, false, false), (true, false, false)),
        ((false, false, true), (true, false, true)),
        ((false, true, false), (true, true, false)),
        ((false, true, true), (true, true, true)),
        ((false, false, false), (false, true, false)),
        ((true, false, false), (true, true, false)),
        ((false, false, true), (false, true, true)),
        ((true, false, true), (true, true, true)),
        ((false, false, false), (false, false, true)),
        ((true, false, false), (true, false, true)),
        ((false, true, false), (false, true, true)),
        ((true, true, false), (true, true, true)),
    ] {
        out.push(LineVertex { pos: c(a.0, a.1, a.2).to_array(), color });
        out.push(LineVertex { pos: c(b.0, b.1, b.2).to_array(), color });
    }
}

/// Sky colour and sky-light brightness for a world time (0–24000 ticks),
/// following the shape of the 1.8 day cycle.
pub fn sky(day_time: i64) -> ([f32; 3], f32) {
    let t = (day_time.rem_euclid(24000)) as f32 / 24000.0;
    // Celestial angle: 0 at noon offset as in vanilla (time 6000 = noon).
    let mut angle = t - 0.25;
    if angle < 0.0 {
        angle += 1.0;
    }
    let angle = angle + ((1.0 - ((angle * std::f32::consts::PI).cos() + 1.0) / 2.0) - angle) / 3.0;
    let brightness = ((angle * std::f32::consts::TAU).cos() * 2.0 + 0.5).clamp(0.0, 1.0);
    let day = Vec3::new(0.62, 0.75, 1.0);
    let night = Vec3::new(0.02, 0.025, 0.06);
    let color = night.lerp(day, brightness);
    (color.to_array(), 0.2 + brightness * 0.8)
}

/// A device without a window, for headless rendering and tests. Returns
/// an error string when no adapter is usable.
pub async fn headless_device() -> Result<(wgpu::Device, wgpu::Queue, wgpu::AdapterInfo), String> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: None,
            apply_limit_buckets: false,
        })
        .await
        .map_err(|e| format!("no GPU adapter: {e}"))?;
    let info = adapter.get_info();
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor {
            label: Some("RustCraft"),
            required_features: wgpu::Features::empty(),
            required_limits: adapter.limits(),
            ..Default::default()
        })
        .await
        .map_err(|e| format!("could not open GPU device: {e}"))?;
    Ok((device, queue, info))
}

/// Renders to an offscreen texture and returns RGBA8 pixels.
pub fn render_to_image(renderer: &mut Renderer, scene: &Scene<'_>, width: u32, height: u32) -> Vec<u8> {
    let device = renderer.device.clone();
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("offscreen"),
        size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: renderer.format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    renderer.resize(width, height);
    let view = target.create_view(&Default::default());
    let mut encoder = device.create_command_encoder(&Default::default());
    renderer.render(&mut encoder, &view, scene);
    let row = (width * 4).div_ceil(256) * 256;
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback"),
        size: (row * height) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    encoder.copy_texture_to_buffer(
        target.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: Some(height) },
        },
        wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
    );
    renderer.queue.submit([encoder.finish()]);
    let slice = readback.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    let _ = device.poll(wgpu::PollType::wait_indefinitely());
    let data = slice.get_mapped_range().expect("readback buffer is mapped");
    let mut out = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        out.extend_from_slice(&data[(y * row) as usize..(y * row + width * 4) as usize]);
    }
    if matches!(renderer.format, wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb) {
        for px in out.chunks_mut(4) {
            px.swap(0, 2);
        }
    }
    out
}

/// Section coordinates containing a world position.
pub fn section_of(pos: DVec3) -> IVec3 {
    (pos.floor().as_ivec3()) >> 4
}
