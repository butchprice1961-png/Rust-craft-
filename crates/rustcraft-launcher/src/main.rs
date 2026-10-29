//! RustCraft launcher and game supervisor.
//!
//! This process lives for the whole session. Its window closes once the game
//! is running; the loop in `main` then blocks on the game's process handle
//! (no polling) and reopens the window when the game exits, with the result
//! of the session. See docs/ARCHITECTURE.md.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod config;
mod core;
mod game;
mod theme;
mod views;

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use eframe::egui;

use rustcraft_core::DataDir;
use rustcraft_platform::InstanceLock;

use crate::app::LauncherApp;
use crate::core::Core;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--version") {
        println!("rustcraft {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }
    // `--smoke-test <file.png>`: open the UI, save a screenshot, exit.
    let smoke = args.iter().position(|a| a == "--smoke-test").map(|i| {
        PathBuf::from(
            args.get(i + 1)
                .map_or("rustcraft-smoke.png", String::as_str),
        )
    });
    // `--smoke-tab <name>` picks the screen a smoke test captures;
    // `--smoke-launch` first plays the selected instance offline.
    let smoke_tab = args
        .iter()
        .position(|a| a == "--smoke-tab")
        .and_then(|i| args.get(i + 1))
        .cloned();
    let mut smoke_launch = args.iter().any(|a| a == "--smoke-launch");

    let dirs = match DataDir::from_env() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("RustCraft cannot find a data directory: {e}");
            return ExitCode::FAILURE;
        }
    };
    init_logging(&dirs);
    tracing::info!(
        "RustCraft {} starting, data in {}",
        env!("CARGO_PKG_VERSION"),
        dirs.root().display()
    );

    let _lock = match InstanceLock::try_acquire(&dirs.root().join("launcher.lock")) {
        Ok(Some(lock)) => Some(lock),
        Ok(None) => {
            tracing::info!("another RustCraft is running; exiting");
            show_already_running();
            return ExitCode::SUCCESS;
        }
        Err(e) => {
            tracing::warn!("could not take the single-instance lock ({e}); continuing");
            None
        }
    };

    let core = match Core::new(dirs) {
        Ok(c) => Arc::new(c),
        Err(e) => {
            tracing::error!("startup failed: {e}");
            return ExitCode::FAILURE;
        }
    };

    loop {
        let smoke = smoke.clone().map(|output| Smoke {
            output,
            tab: smoke_tab.clone(),
            launch: std::mem::take(&mut smoke_launch),
        });
        if let Err(e) = run_window(&core, smoke) {
            tracing::error!("launcher window failed: {e}");
            if core.game.lock().unwrap().is_none() {
                eprintln!("RustCraft could not open its window: {e}");
                return ExitCode::FAILURE;
            }
        }
        let Some(mut running) = core.game.lock().unwrap().take() else {
            break;
        };
        tracing::info!("window closed; supervising {}", running.instance);
        let status = running.child.wait();
        let outcome = running.outcome(status);
        tracing::info!("{}", outcome.summary());
        *core.last_outcome.lock().unwrap() = Some(outcome);
    }
    tracing::info!("RustCraft exiting");
    ExitCode::SUCCESS
}

fn native_options(renderer: eframe::Renderer) -> eframe::NativeOptions {
    eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("RustCraft")
            .with_app_id("dev.rustcraft.RustCraft")
            .with_inner_size([1140.0, 740.0])
            .with_min_inner_size([880.0, 580.0])
            .with_icon(Arc::new(icon())),
        renderer,
        ..Default::default()
    }
}

/// Opens the launcher window; falls back to OpenGL if the GPU renderer
/// cannot start (old drivers, VMs, remote desktops).
/// Options for the automated UI checks run against packaged builds.
#[derive(Clone)]
pub struct Smoke {
    pub output: PathBuf,
    pub tab: Option<String>,
    pub launch: bool,
}

fn run_window(core: &Arc<Core>, smoke: Option<Smoke>) -> eframe::Result {
    let creator = |core: Arc<Core>, smoke: Option<Smoke>| -> eframe::AppCreator<'static> {
        Box::new(move |cc| Ok(Box::new(LauncherApp::new(cc, core, smoke))))
    };
    let first = eframe::run_native(
        "RustCraft",
        native_options(eframe::Renderer::Wgpu),
        creator(core.clone(), smoke.clone()),
    );
    match first {
        Err(e) if core.game.lock().unwrap().is_none() => {
            tracing::warn!("GPU renderer failed ({e}); retrying with OpenGL");
            eframe::run_native(
                "RustCraft",
                native_options(eframe::Renderer::Glow),
                creator(core.clone(), smoke),
            )
        }
        other => other,
    }
}

fn show_already_running() {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("RustCraft")
            .with_inner_size([440.0, 150.0])
            .with_resizable(false),
        ..Default::default()
    };
    let result = eframe::run_ui_native("RustCraft", options, |ui, _| {
        egui::CentralPanel::default_margins().show(ui, |ui| {
            ui.heading("RustCraft is already running");
            ui.label("If a game is open, the launcher comes back when you quit it.");
            if ui.button("OK").clicked() {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
            }
        });
    });
    if result.is_err() {
        eprintln!("RustCraft is already running.");
    }
}

fn init_logging(dirs: &DataDir) {
    use tracing_subscriber::fmt::writer::MakeWriterExt;
    let logs = dirs.logs();
    let _ = std::fs::create_dir_all(&logs);
    let _ = std::fs::rename(logs.join("launcher.log"), logs.join("launcher.prev.log"));
    let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        "info,wgpu_core=warn,wgpu_hal=warn,naga=warn,eframe=warn,egui_wgpu=warn".into()
    });
    let builder = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_ansi(false);
    match std::fs::File::create(logs.join("launcher.log")) {
        Ok(file) => builder
            .with_writer(std::io::stderr.and(Arc::new(file)))
            .init(),
        Err(_) => builder.with_writer(std::io::stderr).init(),
    }
}

/// The window icon, drawn in code so no image asset is needed.
fn icon() -> egui::IconData {
    const N: usize = 64;
    let mut rgba = vec![0u8; N * N * 4];
    for y in 0..N {
        for x in 0..N {
            // Rounded square in the accent colour with a darker inset block.
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            let r = 14.0;
            let cx = fx.clamp(r, N as f32 - r);
            let cy = fy.clamp(r, N as f32 - r);
            if (fx - cx).hypot(fy - cy) > r {
                continue;
            }
            let inset = (16..48).contains(&x) && (16..48).contains(&y);
            let quadrant = (x < 32) == (y < 32);
            let (r, g, b) = match (inset, quadrant) {
                (true, true) => (0xc0, 0x5c, 0x2c),
                (true, false) => (0xf2, 0x8a, 0x55),
                _ => (0xe8, 0x74, 0x3b),
            };
            rgba[(y * N + x) * 4..][..4].copy_from_slice(&[r, g, b, 255]);
        }
    }
    egui::IconData {
        rgba,
        width: N as u32,
        height: N as u32,
    }
}
