// Release builds on Windows run without a console window.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod clipboard;
mod icon;
mod loader;
mod navigator;
mod renderer;
mod viewer;

use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Instant;
use winit::event_loop::{ControlFlow, EventLoop};

static START: OnceLock<Instant> = OnceLock::new();

/// Milliseconds since the start of `main`, for startup profiling logs.
pub fn ms_since_start() -> u128 {
    START.get().map(|t| t.elapsed().as_millis()).unwrap_or(0)
}

fn main() -> anyhow::Result<()> {
    let _ = START.set(Instant::now());
    env_logger::init();

    let Some(path) = std::env::args_os().nth(1).map(PathBuf::from) else {
        eprintln!("usage: pix <image-file>");
        std::process::exit(2);
    };

    // Vulkan loader/layer initialisation is slow and independent of the
    // window, so overlap it with event loop + window creation.
    let gpu_instance = std::thread::spawn(renderer::create_instance);

    let event_loop = EventLoop::new()?;
    // Sleep until something happens; redraws are requested explicitly.
    event_loop.set_control_flow(ControlFlow::Wait);
    log::info!("[{:>4} ms] event loop ready", ms_since_start());

    let mut app = app::App::new(path, event_loop.create_proxy());
    app.set_instance(gpu_instance);
    // Start decoding now so it overlaps with window and GPU startup.
    app.prefetch();
    log::info!("[{:>4} ms] prefetch started", ms_since_start());
    event_loop.run_app(&mut app)?;
    Ok(())
}
