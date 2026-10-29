use crate::drawing::Instance;
use anyhow::{Context, Result, bail};
use egui::{ClippedPrimitive, TexturesDelta, Vec2};
use raw_window_handle::{
    RawDisplayHandle, RawWindowHandle, WaylandDisplayHandle, WaylandWindowHandle,
};
use std::ptr::NonNull;
use wayland_client::{Connection, Proxy, protocol::wl_surface::WlSurface};
use wgpu::util::DeviceExt;

pub struct Renderer {
    // Drop the GPU surface before App drops the Wayland surface/connection.
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    pipeline: wgpu::RenderPipeline,
    uniform: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    instances: wgpu::Buffer,
    capacity: usize,
    egui: egui_wgpu::Renderer,
}

impl Renderer {
    pub fn new(conn: &Connection, window: &WlSurface) -> Result<Self> {
        let backends = wgpu::Backends::VULKAN;
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends,
            ..Default::default()
        });
        let display =
            NonNull::new(conn.backend().display_ptr().cast()).context("Null Wayland display")?;
        let window = NonNull::new(window.id().as_ptr().cast()).context("Null Wayland surface")?;
        // SAFETY: App owns both the connection and wl_surface and drops Renderer first.
        // All Wayland/GPU presentation is performed on the event loop thread.
        let surface = unsafe {
            instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle {
                raw_display_handle: RawDisplayHandle::Wayland(WaylandDisplayHandle::new(display)),
                raw_window_handle: RawWindowHandle::Wayland(WaylandWindowHandle::new(window)),
            })?
        };
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
        }))
        .context("No compatible Vulkan GPU. Install the Vulkan driver for your graphics card")?;
        eprintln!(
            "LivePaint GPU: {} ({:?})",
            adapter.get_info().name,
            adapter.get_info().backend
        );
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                label: Some("LivePaint"),
                required_limits:
                    wgpu::Limits::downlevel_defaults().using_resolution(adapter.limits()),
                ..Default::default()
            }))?;
        let caps = surface.get_capabilities(&adapter);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| {
                matches!(
                    f,
                    wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Rgba8Unorm
                )
            })
            .or_else(|| caps.formats.first().copied())
            .context("No surface format")?;
        if !caps
            .alpha_modes
            .contains(&wgpu::CompositeAlphaMode::PreMultiplied)
        {
            bail!("The GPU backend does not support premultiplied transparent Wayland surfaces");
        }
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: 1,
            height: 1,
            present_mode: wgpu::PresentMode::Fifo,
            desired_maximum_frame_latency: 2,
            alpha_mode: wgpu::CompositeAlphaMode::PreMultiplied,
            view_formats: vec![],
        };
        let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("logical viewport"),
            contents: bytemuck::cast_slice(&[1_f32, 1., 0., 0.]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let bind_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("viewport"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &bind_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform.as_entire_binding(),
            }],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[&bind_layout],
            push_constant_ranges: &[],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("analytic ink"),
            source: wgpu::ShaderSource::Wgsl(include_str!("ink.wgsl").into()),
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("ink"), layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader, entry_point: Some("vs_main"), compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Instance>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4, 2 => Float32x4, 3 => Float32x4],
                }],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader, entry_point: Some("fs_main"), compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState { format, blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING), write_mask: wgpu::ColorWrites::ALL })],
            }),
            primitive: Default::default(), depth_stencil: None, multisample: Default::default(), multiview: None, cache: None,
        });
        let capacity = 256;
        let instances = instance_buffer(&device, capacity);
        let egui = egui_wgpu::Renderer::new(
            &device,
            format,
            egui_wgpu::RendererOptions {
                dithering: false,
                ..Default::default()
            },
        );
        Ok(Self {
            surface,
            device,
            queue,
            config,
            pipeline,
            uniform,
            bind_group,
            instances,
            capacity,
            egui,
        })
    }

    pub fn resize(&mut self, size: Vec2, scale: f32) {
        self.config.width = (size.x * scale).round().max(1.) as u32;
        self.config.height = (size.y * scale).round().max(1.) as u32;
        self.surface.configure(&self.device, &self.config);
        self.queue.write_buffer(
            &self.uniform,
            0,
            bytemuck::cast_slice(&[size.x, size.y, 0., 0.]),
        );
    }

    pub fn render(
        &mut self,
        ink: &[Instance],
        jobs: &[ClippedPrimitive],
        textures: &TexturesDelta,
        scale: f32,
    ) -> Result<()> {
        for (id, delta) in &textures.set {
            self.egui
                .update_texture(&self.device, &self.queue, *id, delta);
        }
        let frame = match self.surface.get_current_texture() {
            Ok(frame) => frame,
            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                self.surface.configure(&self.device, &self.config);
                self.surface
                    .get_current_texture()
                    .context("Cannot recover GPU surface")?
            }
            Err(e) => return Err(e.into()),
        };
        if ink.len() > self.capacity {
            self.capacity = ink.len().next_power_of_two();
            self.instances = instance_buffer(&self.device, self.capacity);
        }
        if !ink.is_empty() {
            self.queue
                .write_buffer(&self.instances, 0, bytemuck::cast_slice(ink));
        }
        let view = frame.texture.create_view(&Default::default());
        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [self.config.width, self.config.height],
            pixels_per_point: scale,
        };
        let mut encoder = self.device.create_command_encoder(&Default::default());
        let extra =
            self.egui
                .update_buffers(&self.device, &self.queue, &mut encoder, jobs, &screen);
        {
            let mut pass = encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("transparent overlay"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        resolve_target: None,
                        depth_slice: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                })
                .forget_lifetime();
            if !ink.is_empty() {
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(0, &self.bind_group, &[]);
                pass.set_vertex_buffer(0, self.instances.slice(..));
                pass.draw(0..6, 0..ink.len() as u32);
            }
            self.egui.render(&mut pass, jobs, &screen);
        }
        self.queue
            .submit(extra.into_iter().chain(std::iter::once(encoder.finish())));
        frame.present();
        for id in &textures.free {
            self.egui.free_texture(id);
        }
        Ok(())
    }
}

fn instance_buffer(device: &wgpu::Device, capacity: usize) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("ink instances"),
        size: (capacity * std::mem::size_of::<Instance>()) as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}
