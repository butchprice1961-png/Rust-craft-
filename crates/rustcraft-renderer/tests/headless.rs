//! Renders generated terrain offscreen and checks the image. Needs a GPU
//! adapter (a software one such as Mesa's lavapipe works); without one the
//! test is skipped unless RUSTCRAFT_REQUIRE_GPU=1. Set RUSTCRAFT_SHOT=<png>
//! to keep the rendered frame.

use glam::{DVec3, IVec3};
use rustcraft_renderer::{Camera, Renderer, Scene, Snapshot, TextureData, mesh, render_to_image, sky};
use rustcraft_world::World;
use rustcraft_world::generate::Generator;

#[test]
fn renders_generated_terrain() {
    let (device, queue, info) = match pollster::block_on(rustcraft_renderer::headless_device()) {
        Ok(d) => d,
        Err(e) if std::env::var("RUSTCRAFT_REQUIRE_GPU").as_deref() != Ok("1") => {
            eprintln!("skipping: {e}");
            return;
        }
        Err(e) => panic!("{e}"),
    };
    eprintln!("adapter: {} ({:?})", info.name, info.backend);

    let generator = Generator::new(2024);
    let (sx, sy, sz) = generator.spawn_point();
    let mut world = World::new(true);
    let r = 4;
    for cx in (sx >> 4) - r..=(sx >> 4) + r {
        for cz in (sz >> 4) - r..=(sz >> 4) + r {
            world.insert_chunk(generator.generate(cx, cz));
        }
    }
    let textures = TextureData::load(&[]);
    let (w, h) = (640, 360);
    let mut renderer = Renderer::new(device, queue, wgpu::TextureFormat::Rgba8Unorm, w, h, &textures);
    let colors = textures.biome_colors();
    let mut meshed = 0;
    for chunk in world.chunks() {
        for sy in 0..16 {
            if let Some(snap) = Snapshot::capture(&world, IVec3::new(chunk.x, sy, chunk.z), &colors) {
                renderer.upload(&mesh::mesh(&snap, &textures.lookup));
                meshed += 1;
            }
        }
    }
    assert!(meshed > 50, "only {meshed} sections meshed");

    let (sky_color, brightness) = sky(6000);
    let camera = Camera {
        pos: DVec3::new(sx as f64 + 0.5, sy as f64 + 12.0, sz as f64 + 0.5),
        yaw: 30.0,
        pitch: 25.0,
        fov_y: 70.0,
        aspect: w as f32 / h as f32,
        far: 160.0,
    };
    let scene =
        Scene { camera, sky_color, sky_brightness: brightness, render_distance: 4, selection: None, lines: &[] };
    let pixels = render_to_image(&mut renderer, &scene, w, h);
    assert_eq!(pixels.len(), (w * h * 4) as usize);
    if let Ok(path) = std::env::var("RUSTCRAFT_SHOT") {
        image::save_buffer(&path, &pixels, w, h, image::ExtendedColorType::Rgba8).unwrap();
    }
    assert!(renderer.stats.sections_drawn > 10, "{:?}", renderer.stats);
    assert!(renderer.stats.triangles > 1000, "{:?}", renderer.stats);
    // The lower half looks down at terrain, so most of it is not sky.
    let sky_px = [0, 1, 2].map(|i| (sky_color[i] * 255.0).round() as i32);
    let lower = &pixels[(w * h * 2) as usize..];
    let terrain = lower.chunks(4).filter(|p| (0..3).any(|i| (p[i] as i32 - sky_px[i]).abs() > 6)).count();
    assert!(terrain * 2 > lower.len() / 4, "terrain covers only {terrain} pixels");
}
