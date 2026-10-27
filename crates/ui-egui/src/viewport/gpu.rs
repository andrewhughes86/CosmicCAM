//! wgpu backend for the 3D viewport.
//!
//! Renders into an offscreen multisampled target with its own depth buffer, resolves it
//! into a texture registered with egui, and the viewport paints that texture as an image.

use CosmicDMS_geom::{Mat4, Vec3};
use CosmicDMS_render::{SceneGeometry, Vertex};
use egui_wgpu::RenderState;
use std::sync::{Arc, Mutex};

use wgpu::util::DeviceExt;

/// Pipelines write through an sRGB view so shading and blending happen in linear space...
const COLOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
/// ...while egui samples native textures as gamma-encoded `Rgba8Unorm`.
const STORAGE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
const SAMPLES: u32 = 4;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    view_proj: [[f32; 4]; 4],
    light_dir: [f32; 4],
    bg_top: [f32; 4],
    bg_bottom: [f32; 4],
}

/// Per-frame inputs for [`ViewportGpu::render`].
pub struct FrameParams {
    pub size_px: [u32; 2],
    pub view_proj: Mat4,
    pub light_dir: Vec3,
    pub bg_top: [f32; 4],
    pub bg_bottom: [f32; 4],
}

/// Uploaded geometry. Large buffers are split into chunks the device accepts.
struct Mesh {
    triangles: Vec<(wgpu::Buffer, u32)>,
    lines: Vec<(wgpu::Buffer, u32)>,
}

/// Most vertices in one buffer, whatever the device allows (a multiple of 2 and 3).
const MAX_CHUNK_VERTICES: usize = 6 << 20;

impl Mesh {
    const EMPTY: Self = Self { triangles: Vec::new(), lines: Vec::new() };

    fn upload(device: &wgpu::Device, label: &str, g: &SceneGeometry) -> Self {
        let by_limit = device.limits().max_buffer_size / std::mem::size_of::<Vertex>() as u64;
        let chunk = (usize::try_from(by_limit).unwrap_or(usize::MAX).min(MAX_CHUNK_VERTICES) / 6 * 6).max(6);
        let buffers = |data: &[Vertex], kind: &str| -> Vec<(wgpu::Buffer, u32)> {
            data.chunks(chunk)
                .filter_map(|part| {
                    let count = u32::try_from(part.len()).ok()?;
                    let buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some(&format!("{label} {kind}")),
                        contents: bytemuck::cast_slice(part),
                        usage: wgpu::BufferUsages::VERTEX,
                    });
                    Some((buf, count))
                })
                .collect()
        };
        Self { triangles: buffers(&g.triangles, "triangles"), lines: buffers(&g.lines, "lines") }
    }
}

struct Targets {
    size: [u32; 2],
    msaa: wgpu::TextureView,
    resolve: wgpu::TextureView,
    depth: wgpu::TextureView,
}

pub struct ViewportGpu {
    render_state: RenderState,
    /// The first GPU validation error reported for the device. wgpu would otherwise panic,
    /// which aborts the whole app in the browser.
    error: Arc<Mutex<Option<String>>>,
    /// The format pipelines write: an sRGB view of the storage texture where the device
    /// allows other view formats, else the storage format itself with sRGB encoded in the
    /// shader (WebGL).
    target_format: wgpu::TextureFormat,
    uniforms: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    background: wgpu::RenderPipeline,
    triangles: wgpu::RenderPipeline,
    highlight: wgpu::RenderPipeline,
    lines: wgpu::RenderPipeline,
    scene: Mesh,
    highlighted: Mesh,
    overlay: Mesh,
    /// The simulated stock and the tool, depth-tested with the scene.
    sim_stock: Mesh,
    sim_tool: Mesh,
    targets: Option<Targets>,
    texture_id: Option<egui::TextureId>,
}

impl ViewportGpu {
    pub fn new(render_state: &RenderState) -> Self {
        let device = &render_state.device;
        let error: Arc<Mutex<Option<String>>> = Arc::default();
        let sink = Arc::clone(&error);
        device.on_uncaptured_error(Arc::new(move |e: wgpu::Error| {
            log::error!("wgpu: {e}");
            if let Ok(mut slot) = sink.lock() {
                slot.get_or_insert_with(|| e.to_string().lines().take(3).collect::<Vec<_>>().join(" "));
            }
        }));
        let view_formats =
            render_state.adapter.get_downlevel_capabilities().flags.contains(wgpu::DownlevelFlags::VIEW_FORMATS);
        let target_format = if view_formats { COLOR_FORMAT } else { STORAGE_FORMAT };
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("viewport shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("scene.wgsl").into()),
        });
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("viewport uniforms"),
            size: std::mem::size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("viewport uniforms"),
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
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("viewport uniforms"),
            layout: &bind_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: uniforms.as_entire_binding() }],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("viewport"),
            bind_group_layouts: &[Some(&bind_layout)],
            immediate_size: 0,
        });

        let vertex_layout = Some(wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Vertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x4],
        });
        let blend_target = [Some(wgpu::ColorTargetState {
            format: target_format,
            blend: Some(wgpu::BlendState::ALPHA_BLENDING),
            write_mask: wgpu::ColorWrites::ALL,
        })];
        let multisample = wgpu::MultisampleState { count: SAMPLES, mask: !0, alpha_to_coverage_enabled: false };
        let depth = |write: bool, bias: wgpu::DepthBiasState| wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(write),
            depth_compare: Some(wgpu::CompareFunction::LessEqual),
            stencil: wgpu::StencilState::default(),
            bias,
        };
        let scene_pipeline = |label: &str, topology, depth_state| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_scene"),
                    compilation_options: Default::default(),
                    buffers: std::slice::from_ref(&vertex_layout),
                },
                primitive: wgpu::PrimitiveState { topology, cull_mode: None, ..Default::default() },
                depth_stencil: Some(depth_state),
                multisample,
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_scene"),
                    compilation_options: Default::default(),
                    targets: &blend_target,
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        // Faces sit slightly behind their outline edges.
        let face_bias = wgpu::DepthBiasState { constant: 2, slope_scale: 1.5, clamp: 0.0 };
        let triangles =
            scene_pipeline("viewport triangles", wgpu::PrimitiveTopology::TriangleList, depth(true, face_bias));
        // Picked and hovered faces are redrawn over the part; without the face bias they win
        // the depth test against the biased part triangles beneath them.
        let highlight = scene_pipeline(
            "viewport highlight",
            wgpu::PrimitiveTopology::TriangleList,
            depth(false, wgpu::DepthBiasState::default()),
        );
        let lines = scene_pipeline(
            "viewport lines",
            wgpu::PrimitiveTopology::LineList,
            depth(true, wgpu::DepthBiasState::default()),
        );
        let background = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("viewport background"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_background"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::Always),
                ..depth(false, wgpu::DepthBiasState::default())
            }),
            multisample,
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_background"),
                compilation_options: Default::default(),
                targets: &blend_target,
            }),
            multiview_mask: None,
            cache: None,
        });

        Self {
            render_state: render_state.clone(),
            error,
            target_format,
            uniforms,
            bind_group,
            background,
            triangles,
            highlight,
            lines,
            scene: Mesh::EMPTY,
            highlighted: Mesh::EMPTY,
            overlay: Mesh::EMPTY,
            sim_stock: Mesh::EMPTY,
            sim_tool: Mesh::EMPTY,
            targets: None,
            texture_id: None,
        }
    }

    /// The first GPU error reported, if any; the viewport stops drawing when there is one.
    pub fn error(&self) -> Option<String> {
        self.error.lock().ok().and_then(|e| e.clone())
    }

    /// Replaces the highlighted faces (triangles only).
    pub fn set_highlight(&mut self, g: &SceneGeometry) {
        self.highlighted = Mesh::upload(&self.render_state.device, "highlight", g);
    }

    /// Replaces the depth-tested scene geometry.
    pub fn set_scene(&mut self, g: &SceneGeometry) {
        self.scene = Mesh::upload(&self.render_state.device, "scene", g);
    }

    /// Replaces the simulated stock.
    pub fn set_sim_stock(&mut self, g: &SceneGeometry) {
        self.sim_stock = Mesh::upload(&self.render_state.device, "sim stock", g);
    }

    /// Replaces the simulated tool.
    pub fn set_sim_tool(&mut self, g: &SceneGeometry) {
        self.sim_tool = Mesh::upload(&self.render_state.device, "sim tool", g);
    }

    /// Replaces the geometry drawn on top of the scene (the WCS triad).
    pub fn set_overlay(&mut self, g: &SceneGeometry) {
        self.overlay = Mesh::upload(&self.render_state.device, "overlay", g);
    }

    fn ensure_targets(&mut self, size: [u32; 2]) -> Option<egui::TextureId> {
        let device = &self.render_state.device;
        let target_format = self.target_format;
        let srgb_views = target_format == COLOR_FORMAT;
        let max = device.limits().max_texture_dimension_2d.max(1);
        let size = [size[0].clamp(1, max), size[1].clamp(1, max)];
        if let (Some(t), Some(id)) = (&self.targets, self.texture_id)
            && t.size == size
        {
            return Some(id);
        }
        let texture = |label: &str, format: wgpu::TextureFormat, samples, usage| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: samples,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: if format == STORAGE_FORMAT && srgb_views { &[COLOR_FORMAT] } else { &[] },
            })
        };
        let srgb_view = |t: &wgpu::Texture| {
            t.create_view(&wgpu::TextureViewDescriptor { format: Some(target_format), ..Default::default() })
        };
        let msaa = texture("viewport msaa", STORAGE_FORMAT, SAMPLES, wgpu::TextureUsages::RENDER_ATTACHMENT);
        let color = texture(
            "viewport color",
            STORAGE_FORMAT,
            1,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        );
        let depth = texture("viewport depth", DEPTH_FORMAT, SAMPLES, wgpu::TextureUsages::RENDER_ATTACHMENT);
        let display = color.create_view(&wgpu::TextureViewDescriptor::default());
        let targets = Targets {
            size,
            msaa: srgb_view(&msaa),
            resolve: srgb_view(&color),
            depth: depth.create_view(&wgpu::TextureViewDescriptor::default()),
        };
        let mut renderer = self.render_state.renderer.write();
        let id = match self.texture_id {
            Some(id) => {
                renderer.update_egui_texture_from_wgpu_texture(device, &display, wgpu::FilterMode::Linear, id);
                id
            }
            None => renderer.register_native_texture(device, &display, wgpu::FilterMode::Linear),
        };
        drop(renderer);
        self.targets = Some(targets);
        self.texture_id = Some(id);
        Some(id)
    }

    /// Draws one frame and returns the egui texture holding it.
    pub fn render(&mut self, p: &FrameParams) -> Option<egui::TextureId> {
        let id = self.ensure_targets(p.size_px)?;
        let targets = self.targets.as_ref()?;
        let uniforms = Uniforms {
            view_proj: p.view_proj.to_cols_array_2d(),
            light_dir: p
                .light_dir
                .normalize_or(Vec3::Z)
                .extend(if self.target_format == COLOR_FORMAT { 0.0 } else { 1.0 })
                .to_array(),
            bg_top: p.bg_top,
            bg_bottom: p.bg_bottom,
        };
        let queue = &self.render_state.queue;
        queue.write_buffer(&self.uniforms, 0, bytemuck::bytes_of(&uniforms));

        let mut encoder = self
            .render_state
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("viewport") });
        let pass = |encoder: &mut wgpu::CommandEncoder, first: bool, meshes: &[&Mesh], highlight: Option<&Mesh>| {
            let background = highlight.is_some();
            let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("viewport pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &targets.msaa,
                    depth_slice: None,
                    resolve_target: Some(&targets.resolve),
                    ops: wgpu::Operations {
                        load: if first { wgpu::LoadOp::Clear(wgpu::Color::BLACK) } else { wgpu::LoadOp::Load },
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &targets.depth,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Discard }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            rp.set_bind_group(0, &self.bind_group, &[]);
            if background {
                rp.set_pipeline(&self.background);
                rp.draw(0..3, 0..1);
            }
            let none = Vec::new();
            let lit = highlight.map_or(&none, |h| &h.triangles);
            let mut draw = |pipeline: &wgpu::RenderPipeline, chunks: &Vec<(wgpu::Buffer, u32)>| {
                if !chunks.is_empty() {
                    rp.set_pipeline(pipeline);
                }
                for (buf, n) in chunks {
                    rp.set_vertex_buffer(0, buf.slice(..));
                    rp.draw(0..*n, 0..1);
                }
            };
            for mesh in meshes {
                draw(&self.triangles, &mesh.triangles);
            }
            draw(&self.highlight, lit);
            for mesh in meshes {
                draw(&self.lines, &mesh.lines);
            }
        };
        pass(&mut encoder, true, &[&self.scene, &self.sim_stock, &self.sim_tool], Some(&self.highlighted));
        // Second pass with a fresh depth buffer so the WCS triad is never hidden by the part.
        pass(&mut encoder, false, &[&self.overlay], None);
        queue.submit([encoder.finish()]);
        Some(id)
    }
}

impl Drop for ViewportGpu {
    fn drop(&mut self) {
        if let Some(id) = self.texture_id.take() {
            self.render_state.renderer.write().free_texture(&id);
        }
    }
}
