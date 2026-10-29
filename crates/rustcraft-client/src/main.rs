//! RustCraft's native client for Minecraft: Java Edition 1.8.9 worlds.
//!
//! Plays local worlds (Anvil format, compatible with vanilla 1.8 saves)
//! in creative mode. The launcher starts it with a JSON spec on stdin;
//! developers can use flags (`--help`).

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod game;
mod hud;
mod local;
mod player;
mod settings;
mod spec;

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::OnceLock;

static ADAPTER: OnceLock<String> = OnceLock::new();

pub fn set_adapter_name(name: String) {
    let _ = ADAPTER.set(name);
}

/// Writes a crash report next to the instance's other crash reports, where
/// the launcher looks for it, before the default panic output.
fn install_crash_reporter(game_dir: PathBuf) {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let backtrace = std::backtrace::Backtrace::force_capture();
        let report = format!(
            "---- RustCraft crash report ----\n\nRustCraft {} ({} {})\nGPU: {}\n\n{info}\n\nBacktrace:\n{backtrace}\n",
            env!("CARGO_PKG_VERSION"),
            std::env::consts::OS,
            std::env::consts::ARCH,
            ADAPTER.get().map_or("not initialised", String::as_str),
        );
        let dir = game_dir.join("crash-reports");
        let path = dir.join(format!("crash-{}-client.txt", rustcraft_core::fs::unix_time()));
        if std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(&path, report)).is_ok() {
            eprintln!("crash report saved to {}", path.display());
        }
        default(info);
    }));
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print!("{}", spec::USAGE);
        return ExitCode::SUCCESS;
    }
    if args.iter().any(|a| a == "--version") {
        println!("rustcraft-client {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,wgpu_core=warn,wgpu_hal=warn,naga=warn".into()),
        )
        .with_ansi(false)
        .with_writer(std::io::stderr)
        .init();
    let spec = match spec::LaunchSpec::from_args(&args) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };
    if spec.server.is_some() {
        eprintln!("Multiplayer is not available in this build of the native client yet.");
        return ExitCode::from(2);
    }
    install_crash_reporter(spec.game_dir.clone());
    tracing::info!("RustCraft client {} starting in {}", env!("CARGO_PKG_VERSION"), spec.game_dir.display());

    let event_loop = match winit::event_loop::EventLoop::new() {
        Ok(el) => el,
        Err(e) => {
            eprintln!("RustCraft could not connect to the display: {e}");
            return ExitCode::FAILURE;
        }
    };
    let mut app = app::App::new(spec);
    if let Err(e) = event_loop.run_app(&mut app) {
        eprintln!("RustCraft stopped: {e}");
        return ExitCode::FAILURE;
    }
    match app.result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("RustCraft stopped: {e}");
            ExitCode::FAILURE
        }
    }
}
