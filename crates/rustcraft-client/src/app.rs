//! The window: event handling, the fixed 20 Hz tick loop, rendering the
//! world and the egui interface, and the automation modes (screenshot,
//! benchmark) used to test packaged builds.

use std::io::Write;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, anyhow};
use glam::DVec3;
use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, DeviceId, ElementState, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{CursorGrabMode, Window, WindowId};

use rustcraft_renderer::{Camera, Renderer, Scene, TextureData, render_to_image, sky};

use crate::game::Game;
use crate::hud::{Action, Hud, Menu};
use crate::local::LocalWorld;
use crate::settings::Settings;
use crate::spec::LaunchSpec;

const TICK: f64 = 0.05;

pub struct App {
    spec: LaunchSpec,
    settings: Settings,
    textures: Arc<TextureData>,
    running: Option<Running>,
    started: Instant,
    pub result: Result<(), String>,
}

struct Benchmark {
    until: Option<Instant>,
    frames: Vec<f64>,
    origin: DVec3,
    loaded_after: Duration,
}

struct Running {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    view_format: wgpu::TextureFormat,
    renderer: Renderer,
    egui_ctx: egui::Context,
    egui_state: egui_winit::State,
    egui_renderer: egui_wgpu::Renderer,
    game: Game,
    hud: Hud,
    grabbed: bool,
    last: Instant,
    accumulator: f64,
    ready_sent: bool,
    loaded_frames: u32,
    benchmark: Option<Benchmark>,
}

impl App {
    pub fn new(spec: LaunchSpec) -> Self {
        let mut settings = Settings::load(&spec.game_dir);
        if spec.render_distance != LaunchSpec::default().render_distance {
            settings.render_distance = spec.render_distance;
        }
        let sources: Vec<rustcraft_renderer::Source> = spec
            .resource_packs
            .iter()
            .filter(|p| p.exists())
            .map(|p| rustcraft_renderer::Source::from_path(p))
            .collect();
        let textures = TextureData::load(&sources);
        tracing::info!(
            "textures: {} layers, {} from resource packs, {} px",
            textures.layers.len(),
            textures.from_packs,
            textures.size
        );
        Self { spec, settings, textures: Arc::new(textures), running: None, started: Instant::now(), result: Ok(()) }
    }

    fn start(&mut self, event_loop: &ActiveEventLoop) -> anyhow::Result<Running> {
        let attrs = Window::default_attributes()
            .with_title("RustCraft")
            .with_inner_size(winit::dpi::LogicalSize::new(1280.0, 720.0))
            .with_min_inner_size(winit::dpi::LogicalSize::new(320.0, 240.0));
        let window = Arc::new(event_loop.create_window(attrs).context("could not create the game window")?);

        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_with_display_handle_from_env(Box::new(
            event_loop.owned_display_handle(),
        )));
        let surface = instance.create_surface(window.clone()).context("could not create a drawing surface")?;
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: Some(&surface),
            apply_limit_buckets: false,
        }))
        .map_err(|e| {
            anyhow!("no compatible GPU found ({e}). Update your graphics drivers; see docs/TROUBLESHOOTING.md")
        })?;
        let info = adapter.get_info();
        tracing::info!("GPU: {} ({:?}, {:?})", info.name, info.backend, info.device_type);
        crate::set_adapter_name(format!("{} ({:?})", info.name, info.backend));
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("RustCraft"),
            required_features: wgpu::Features::empty(),
            required_limits: adapter.limits(),
            ..Default::default()
        }))
        .context("could not open the GPU device")?;

        let size = window.inner_size();
        let caps = surface.get_capabilities(&adapter);
        // Prefer a non-sRGB target: vanilla's colour maths is in gamma space,
        // and egui expects it too.
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| !f.is_srgb())
            .or(caps.formats.first().copied())
            .ok_or_else(|| anyhow!("the window cannot be drawn to"))?;
        let view_format = format.remove_srgb_suffix();
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            color_space: wgpu::SurfaceColorSpace::Auto,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: if self.settings.vsync {
                wgpu::PresentMode::AutoVsync
            } else {
                wgpu::PresentMode::AutoNoVsync
            },
            desired_maximum_frame_latency: 2,
            alpha_mode: wgpu::CompositeAlphaMode::Auto,
            view_formats: if view_format != format { vec![view_format] } else { vec![] },
        };
        surface.configure(&device, &config);

        let renderer = Renderer::new(device.clone(), queue, view_format, config.width, config.height, &self.textures);
        let egui_ctx = egui::Context::default();
        let egui_state = egui_winit::State::new(
            egui_ctx.clone(),
            egui::ViewportId::ROOT,
            &window,
            Some(window.scale_factor() as f32),
            None,
            None,
        );
        let egui_renderer = egui_wgpu::Renderer::new(&device, view_format, egui_wgpu::RendererOptions::default());

        let saves = self.spec.game_dir.join("saves");
        let local = LocalWorld::open(&saves, &self.spec.world, self.spec.seed)?;
        let game =
            Game::new(local, self.textures.lookup.clone(), self.textures.biome_colors(), self.settings.render_distance);
        let benchmark = self.spec.benchmark_seconds.map(|_| Benchmark {
            until: None,
            frames: Vec::new(),
            origin: DVec3::ZERO,
            loaded_after: Duration::ZERO,
        });

        let mut running = Running {
            window,
            surface,
            config,
            view_format,
            renderer,
            egui_ctx,
            egui_state,
            egui_renderer,
            game,
            hud: Hud::new(format!("{} ({:?})", info.name, info.backend)),
            grabbed: false,
            last: Instant::now(),
            accumulator: 0.0,
            ready_sent: false,
            loaded_frames: 0,
            benchmark,
        };
        if self.spec.screenshot.is_none() && self.spec.benchmark_seconds.is_none() {
            running.grab(true);
        }
        Ok(running)
    }
}

impl Running {
    fn grab(&mut self, on: bool) {
        if on {
            let grabbed = self
                .window
                .set_cursor_grab(CursorGrabMode::Locked)
                .or_else(|_| self.window.set_cursor_grab(CursorGrabMode::Confined));
            self.grabbed = grabbed.is_ok();
            self.window.set_cursor_visible(!self.grabbed);
        } else {
            let _ = self.window.set_cursor_grab(CursorGrabMode::None);
            self.window.set_cursor_visible(true);
            self.grabbed = false;
        }
    }

    fn set_menu(&mut self, menu: Menu) {
        self.hud.menu = menu;
        self.game.paused = menu != Menu::None;
        self.game.controls = Default::default();
        self.grab(menu == Menu::None);
    }

    fn key(&mut self, event: &KeyEvent, settings: &Settings) {
        let PhysicalKey::Code(code) = event.physical_key else { return };
        let down = event.state == ElementState::Pressed;
        if code == KeyCode::Escape && down && !event.repeat {
            let next = if self.hud.menu == Menu::None { Menu::Pause } else { Menu::None };
            self.set_menu(next);
            return;
        }
        if self.hud.menu != Menu::None {
            return;
        }
        let c = &mut self.game.controls;
        match code {
            KeyCode::KeyW => c.forward = down,
            KeyCode::KeyS => c.back = down,
            KeyCode::KeyA => c.left = down,
            KeyCode::KeyD => c.right = down,
            KeyCode::Space => {
                if down && !c.jump {
                    c.jump_pressed = true;
                }
                c.jump = down;
            }
            KeyCode::ShiftLeft => c.sneak = down,
            KeyCode::ControlLeft => c.sprint = down,
            KeyCode::F3 if down => self.hud.show_debug = !self.hud.show_debug,
            KeyCode::F1 if down => self.hud.hidden = !self.hud.hidden,
            KeyCode::F2 if down && !event.repeat => {
                self.screenshot(settings, None);
            }
            _ if down => {
                const DIGITS: [KeyCode; 9] = [
                    KeyCode::Digit1,
                    KeyCode::Digit2,
                    KeyCode::Digit3,
                    KeyCode::Digit4,
                    KeyCode::Digit5,
                    KeyCode::Digit6,
                    KeyCode::Digit7,
                    KeyCode::Digit8,
                    KeyCode::Digit9,
                ];
                if let Some(slot) = DIGITS.iter().position(|d| *d == code) {
                    self.game.selected = slot;
                }
            }
            _ => {}
        }
    }

    fn scene(
        &self,
        settings: &Settings,
        alpha: f64,
    ) -> (Scene<'static>, Option<(glam::IVec3, Vec<rustcraft_world::block::Aabb>)>) {
        let p = &self.game.player;
        let fov = settings.fov * if p.sprinting { 1.1 } else { 1.0 };
        let camera = Camera {
            pos: self.game.camera_pos(alpha),
            yaw: p.yaw,
            pitch: p.pitch,
            fov_y: fov,
            aspect: self.config.width as f32 / self.config.height.max(1) as f32,
            far: (self.game.render_distance.max(2) * 16 + 32) as f32,
        };
        let (sky_color, sky_brightness) = sky(self.game.day_time);
        let scene = Scene {
            camera,
            sky_color,
            sky_brightness,
            render_distance: self.game.render_distance,
            selection: None,
            lines: &[],
        };
        (scene, self.game.selection_boxes())
    }

    fn screenshot(&mut self, settings: &Settings, path: Option<std::path::PathBuf>) -> Option<std::path::PathBuf> {
        let (w, h) = (self.config.width, self.config.height);
        let (mut scene, selection) = self.scene(settings, 1.0);
        if let Some((pos, boxes)) = &selection {
            scene.selection = Some((*pos, boxes.as_slice()));
        }
        let pixels = render_to_image(&mut self.renderer, &scene, w, h);
        let path = path.unwrap_or_else(|| {
            let dir = std::env::current_dir().unwrap_or_default().join("screenshots");
            dir.join(format!("{}.png", rustcraft_core::fs::unix_time()))
        });
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        match image::save_buffer(&path, &pixels, w, h, image::ExtendedColorType::Rgba8) {
            Ok(()) => {
                tracing::info!("saved screenshot {}", path.display());
                Some(path)
            }
            Err(e) => {
                tracing::error!("could not save screenshot: {e}");
                None
            }
        }
    }

    /// One frame; returns false when the client should exit.
    fn frame(
        &mut self,
        spec: &LaunchSpec,
        settings: &mut Settings,
        textures: &TextureData,
        started: Instant,
    ) -> anyhow::Result<bool> {
        let now = Instant::now();
        let dt = now - self.last;
        self.last = now;
        self.hud.record_frame(dt);
        self.accumulator += dt.as_secs_f64().min(0.25);
        while self.accumulator >= TICK {
            if let Some(b) = &self.benchmark
                && b.until.is_some()
            {
                // Fly a straight, repeatable route east.
                let p = &mut self.game.player;
                let t = b.frames.len() as f64;
                p.prev_pos = p.pos;
                p.pos = b.origin + DVec3::new(t * 0.1, 0.0, 0.0);
                p.yaw = -90.0;
                p.pitch = 20.0;
            } else {
                self.game.tick();
            }
            self.accumulator -= TICK;
        }
        let alpha = self.accumulator / TICK;
        self.game.update(&mut self.renderer);
        self.renderer.animate(self.game.ticks);

        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t) | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => return Ok(true),
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(self.renderer.device(), &self.config);
                return Ok(true);
            }
            other => return Err(anyhow!("the window surface failed: {other:?}")),
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor { format: Some(self.view_format), ..Default::default() });
        let (mut scene, selection) = self.scene(settings, alpha);
        if let Some((pos, boxes)) = &selection
            && !self.hud.hidden
        {
            scene.selection = Some((*pos, boxes.as_slice()));
        }
        let device = self.renderer.device().clone();
        let queue = self.renderer.queue().clone();
        let mut encoder = device.create_command_encoder(&Default::default());
        self.renderer.render(&mut encoder, &view, &scene);

        let raw = self.egui_state.take_egui_input(&self.window);
        let stats = self.renderer.stats;
        let mut action = Action::None;
        let mut output = self.egui_ctx.run_ui(raw, |ui| {
            action = self.hud.ui(ui.ctx(), &mut self.game, textures, stats, settings);
        });
        self.egui_state.handle_platform_output(&self.window, std::mem::take(&mut output.platform_output));
        let jobs = self.egui_ctx.tessellate(std::mem::take(&mut output.shapes), output.pixels_per_point);
        for (id, deltas) in output.textures_delta.set.drain() {
            for delta in &deltas {
                self.egui_renderer.update_texture(&device, &queue, id, delta);
            }
        }
        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [self.config.width, self.config.height],
            pixels_per_point: output.pixels_per_point,
        };
        let extra = self.egui_renderer.update_buffers(&device, &queue, &mut encoder, &jobs, &screen);
        {
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("ui"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            self.egui_renderer.render(&mut pass.forget_lifetime(), &jobs, &screen);
        }
        for id in output.textures_delta.free.drain() {
            self.egui_renderer.free_texture(&id);
        }
        queue.submit(extra.into_iter().chain([encoder.finish()]));
        self.window.pre_present_notify();
        queue.present(frame);

        match action {
            Action::Resume => self.set_menu(Menu::None),
            Action::SaveAndQuit => {
                self.game.save();
                return Ok(false);
            }
            Action::SettingsChanged => {
                self.game.render_distance = settings.render_distance;
                let mode = if settings.vsync { wgpu::PresentMode::AutoVsync } else { wgpu::PresentMode::AutoNoVsync };
                if mode != self.config.present_mode {
                    self.config.present_mode = mode;
                    self.surface.configure(&device, &self.config);
                }
                settings.save(&spec.game_dir);
            }
            Action::None => {}
        }

        if !self.ready_sent && self.game.spawn_area_ready() {
            // Tells the launcher the game is up, so it can hide its window.
            println!("RUSTCRAFT_READY");
            let _ = std::io::stdout().flush();
            self.ready_sent = true;
            tracing::info!("playable {:.2}s after start", started.elapsed().as_secs_f64());
        }
        if self.game.fully_loaded() {
            self.loaded_frames += 1;
        }
        if let Some(path) = &spec.screenshot
            && self.loaded_frames >= 10
        {
            self.screenshot(settings, Some(path.clone())).ok_or_else(|| anyhow!("could not save the screenshot"))?;
            self.game.save();
            return Ok(false);
        }
        if let (Some(b), Some(secs)) = (&mut self.benchmark, spec.benchmark_seconds) {
            match b.until {
                None if self.loaded_frames >= 10 => {
                    b.until = Some(Instant::now() + Duration::from_secs(secs as u64));
                    b.origin = self.game.player.pos + DVec3::new(0.0, 30.0, 0.0);
                    b.loaded_after = started.elapsed();
                    self.game.player.flying = true;
                }
                Some(until) => {
                    b.frames.push(dt.as_secs_f64() * 1000.0);
                    if Instant::now() >= until {
                        report_benchmark(b, &self.hud.adapter, &self.renderer, spec, settings);
                        return Ok(false);
                    }
                }
                None => {}
            }
        }
        if !settings.vsync && spec.benchmark_seconds.is_none() {
            let budget = Duration::from_secs_f64(1.0 / settings.max_fps as f64);
            if let Some(rest) = budget.checked_sub(now.elapsed()) {
                std::thread::sleep(rest);
            }
        }
        Ok(true)
    }
}

fn report_benchmark(b: &Benchmark, adapter: &str, renderer: &Renderer, spec: &LaunchSpec, settings: &Settings) {
    let mut sorted = b.frames.clone();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let n = sorted.len().max(1);
    let total: f64 = sorted.iter().sum();
    let p99 = sorted[((n as f64 * 0.99) as usize).min(n - 1)];
    // "1% low": average FPS over the slowest 1% of frames.
    let slow = &sorted[n - (n / 100).max(1)..];
    let low = 1000.0 / (slow.iter().sum::<f64>() / slow.len() as f64);
    let report = serde_json::json!({
        "rustcraft": env!("CARGO_PKG_VERSION"),
        "gpu": adapter,
        "render_distance": settings.render_distance,
        "vsync": settings.vsync,
        "world": spec.world,
        "seconds_to_loaded": b.loaded_after.as_secs_f64(),
        "frames": n,
        "avg_fps": 1000.0 * n as f64 / total,
        "one_percent_low_fps": low,
        "p50_frame_ms": sorted[n / 2],
        "p99_frame_ms": p99,
        "max_frame_ms": sorted[n - 1],
        "sections_drawn": renderer.stats.sections_drawn,
        "triangles": renderer.stats.triangles,
        "gpu_mesh_mib": renderer.stats.gpu_bytes as f64 / 1048576.0,
    });
    println!("{report}");
    let dir = spec.game_dir.join("benchmarks");
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::fs::write(dir.join(format!("{}.json", rustcraft_core::fs::unix_time())), report.to_string());
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.running.is_some() {
            return;
        }
        match self.start(event_loop) {
            Ok(r) => self.running = Some(r),
            Err(e) => {
                self.result = Err(format!("{e:#}"));
                event_loop.exit();
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(r) = self.running.as_mut() else { return };
        let response = r.egui_state.on_window_event(&r.window, &event);
        match event {
            WindowEvent::CloseRequested => {
                r.game.save();
                event_loop.exit();
            }
            WindowEvent::Resized(size) if size.width > 0 && size.height > 0 => {
                r.config.width = size.width;
                r.config.height = size.height;
                r.surface.configure(r.renderer.device(), &r.config);
                r.renderer.resize(size.width, size.height);
            }
            WindowEvent::Focused(false)
                if r.hud.menu == Menu::None
                    && self.spec.benchmark_seconds.is_none()
                    && self.spec.screenshot.is_none() =>
            {
                r.set_menu(Menu::Pause)
            }
            WindowEvent::KeyboardInput { event, .. } => r.key(&event, &self.settings),
            WindowEvent::MouseInput { state, button, .. } if r.hud.menu == Menu::None && !response.consumed => {
                if !r.grabbed {
                    if state == ElementState::Pressed {
                        r.grab(true);
                    }
                    return;
                }
                let down = state == ElementState::Pressed;
                let c = &mut r.game.controls;
                match button {
                    MouseButton::Left => {
                        c.attack = down;
                        c.attack_pressed |= down;
                    }
                    MouseButton::Right => c.use_pressed |= down,
                    MouseButton::Middle => c.pick_pressed |= down,
                    _ => {}
                }
            }
            WindowEvent::MouseWheel { delta, .. } if r.hud.menu == Menu::None => {
                let lines = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y,
                    MouseScrollDelta::PixelDelta(p) => (p.y / 40.0) as f32,
                };
                if lines.abs() >= 1.0 {
                    let step = if lines > 0.0 { 8 } else { 1 };
                    r.game.selected = (r.game.selected + step) % 9;
                }
            }
            WindowEvent::RedrawRequested => match r.frame(&self.spec, &mut self.settings, &self.textures, self.started)
            {
                Ok(true) => {}
                Ok(false) => event_loop.exit(),
                Err(e) => {
                    self.result = Err(format!("{e:#}"));
                    r.game.save();
                    event_loop.exit();
                }
            },
            _ => {}
        }
    }

    fn device_event(&mut self, _: &ActiveEventLoop, _: DeviceId, event: DeviceEvent) {
        let Some(r) = self.running.as_mut() else { return };
        if let DeviceEvent::MouseMotion { delta } = event
            && r.grabbed
            && r.hud.menu == Menu::None
        {
            let s = 0.15 * self.settings.sensitivity;
            let p = &mut r.game.player;
            p.yaw += delta.0 as f32 * s;
            p.pitch = (p.pitch + delta.1 as f32 * s).clamp(-90.0, 90.0);
        }
    }

    fn about_to_wait(&mut self, _: &ActiveEventLoop) {
        if let Some(r) = &self.running {
            r.window.request_redraw();
        }
    }
}
