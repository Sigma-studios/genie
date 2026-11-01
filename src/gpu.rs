//! One GPU device for the life of the daemon, one swapchain per show. Creating
//! a device takes long enough to be seen, a swapchain doesn't.

use egui_wgpu::Renderer;
use egui_wgpu::RendererOptions;
use egui_wgpu::ScreenDescriptor;
use egui_wgpu::wgpu;
use raw_window_handle::RawDisplayHandle;
use raw_window_handle::RawWindowHandle;
use raw_window_handle::WaylandDisplayHandle;
use raw_window_handle::WaylandWindowHandle;
use std::ptr::NonNull;
use wayland_client::Connection;
use wayland_client::Proxy;
use wayland_client::protocol::wl_surface::WlSurface;

pub struct Gpu {
    renderer: Renderer,
    device: wgpu::Device,
    queue: wgpu::Queue,
    instance: wgpu::Instance,
    format: wgpu::TextureFormat,
    present_mode: wgpu::PresentMode,
    alpha_mode: wgpu::CompositeAlphaMode,
}

pub struct Target {
    /// Before the wl_surface it's made from, which its owner drops after this.
    surface: wgpu::Surface<'static>,
    size: (u32, u32),
}

fn make_surface(instance: &wgpu::Instance, conn: &Connection, wl_surface: &WlSurface) -> wgpu::Surface<'static> {
    let display = NonNull::new(conn.backend().display_ptr() as *mut _).expect("no Wayland display");
    let window = NonNull::new(wl_surface.id().as_ptr() as *mut _).expect("no Wayland surface");
    // SAFETY: both outlive the surface: the connection is the daemon's, and
    // Target is dropped before its wl_surface.
    unsafe {
        instance
            .create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle {
                raw_display_handle: Some(RawDisplayHandle::Wayland(WaylandDisplayHandle::new(display))),
                raw_window_handle: RawWindowHandle::Wayland(WaylandWindowHandle::new(window)),
            })
            .expect("couldn't create a GPU surface")
    }
}

impl Gpu {
    /// `probe` is any surface, to learn what formats a swapchain can have.
    pub fn new(conn: &Connection, probe: &WlSurface) -> Self {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::PRIMARY,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let surface = make_surface(&instance, conn, probe);
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            // The GPU driving the screens, on a desktop with a discrete one.
            // Drawing on another means sharing every frame across GPUs, and
            // an Intel chip beside an NVIDIA card shows glitches doing that.
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            ..Default::default()
        }))
        .expect("no suitable GPU");
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            memory_hints: wgpu::MemoryHints::MemoryUsage,
            ..Default::default()
        }))
        .expect("couldn't open the GPU");
        let capabilities = surface.get_capabilities(&adapter);
        // egui blends in gamma space, and expects a target that doesn't convert.
        // Eight bits of alpha too: the first format can be 10-bit colour with
        // two of alpha, which turns the shadow into steps.
        let eight_bits = [wgpu::TextureFormat::Bgra8Unorm, wgpu::TextureFormat::Rgba8Unorm];
        let format = eight_bits
            .into_iter()
            .find(|f| capabilities.formats.contains(f))
            .or_else(|| capabilities.formats.iter().copied().find(|f| !f.is_srgb()))
            .unwrap_or(capabilities.formats[0]);
        let present_mode = if capabilities.present_modes.contains(&wgpu::PresentMode::Mailbox) {
            wgpu::PresentMode::Mailbox
        } else {
            wgpu::PresentMode::Fifo
        };
        let alpha_mode = if capabilities.alpha_modes.contains(&wgpu::CompositeAlphaMode::PreMultiplied) {
            wgpu::CompositeAlphaMode::PreMultiplied
        } else {
            log::warn!("no premultiplied alpha: the shadow won't look right");
            capabilities.alpha_modes[0]
        };
        let renderer = Renderer::new(&device, format, RendererOptions { msaa_samples: 1, ..Default::default() });
        log::info!("drawing with {:?} on {}, {format:?}", adapter.get_info().backend, adapter.get_info().name);
        drop(surface);
        Self { renderer, device, queue, instance, format, present_mode, alpha_mode }
    }

    pub fn target(&self, conn: &Connection, wl_surface: &WlSurface) -> Target {
        Target { surface: make_surface(&self.instance, conn, wl_surface), size: (0, 0) }
    }

    fn configure(&self, target: &mut Target, size: (u32, u32)) {
        target.size = size;
        target.surface.configure(
            &self.device,
            &wgpu::SurfaceConfiguration {
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                format: self.format,
                width: size.0,
                height: size.1,
                present_mode: self.present_mode,
                alpha_mode: self.alpha_mode,
                color_space: wgpu::SurfaceColorSpace::Auto,
                view_formats: vec![],
                desired_maximum_frame_latency: 2,
            },
        );
    }

    fn acquire(&self, target: &mut Target) -> Option<wgpu::SurfaceTexture> {
        match target.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(texture) => return Some(texture),
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => return None,
            _ => self.configure(target, target.size),
        }
        match target.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(texture) | wgpu::CurrentSurfaceTexture::Suboptimal(texture) => Some(texture),
            other => {
                log::warn!("skipping a frame: {other:?}");
                None
            }
        }
    }

    /// Draws a frame, which also commits the wl_surface. Size in pixels.
    pub fn render(&mut self, target: &mut Target, ctx: &egui::Context, output: egui::FullOutput, size: (u32, u32)) -> bool {
        let screen = ScreenDescriptor { size_in_pixels: [size.0, size.1], pixels_per_point: output.pixels_per_point };
        // Textures must reach the renderer even if this frame doesn't.
        for (id, deltas) in &output.textures_delta.set {
            for delta in deltas {
                self.renderer.update_texture(&self.device, &self.queue, *id, delta);
            }
        }
        if target.size != size {
            self.configure(target, size);
        }
        let texture = self.acquire(target);
        let mut presented = false;
        if let Some(texture) = texture {
            let view = texture.texture.create_view(&wgpu::TextureViewDescriptor::default());
            let triangles = ctx.tessellate(output.shapes, output.pixels_per_point);
            let mut encoder = self.device.create_command_encoder(&Default::default());
            self.renderer.update_buffers(&self.device, &self.queue, &mut encoder, &triangles, &screen);
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("genie"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            self.renderer.render(&mut pass.forget_lifetime(), &triangles, &screen);
            self.queue.submit(Some(encoder.finish()));
            self.queue.present(texture);
            presented = true;
        }
        for id in &output.textures_delta.free {
            self.renderer.free_texture(id);
        }
        presented
    }
}
