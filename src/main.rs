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
use winit::event_loop::{ControlFlow, EventLoop};

fn main() -> anyhow::Result<()> {
    env_logger::init();

    let Some(path) = std::env::args_os().nth(1).map(PathBuf::from) else {
        eprintln!("usage: pix <image-file>");
        std::process::exit(2);
    };

    let event_loop = EventLoop::new()?;
    // Sleep until something happens; redraws are requested explicitly.
    event_loop.set_control_flow(ControlFlow::Wait);

    let mut app = app::App::new(path, event_loop.create_proxy());
    event_loop.run_app(&mut app)?;
    Ok(())
}
