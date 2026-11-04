//! winit application: translates input into viewer/navigator actions and
//! schedules redraws only when something changes.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use winit::application::ApplicationHandler;
use winit::dpi::PhysicalPosition;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoopProxy};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Fullscreen, Window, WindowId};

use crate::clipboard;
use crate::loader::{Decoded, Loader};
use crate::navigator::Navigator;
use crate::renderer::Renderer;
use crate::viewer::Viewer;

pub struct App {
    start_path: PathBuf,
    window: Option<Arc<Window>>,
    renderer: Option<Renderer>,
    viewer: Viewer,
    navigator: Option<Navigator>,
    loader: Loader,
    current: Option<Arc<Decoded>>,
    cursor: (f32, f32),
    panning: bool,
    last_frame: Instant,
    /// Direction of the last arrow press (+1/-1), used to skip broken files.
    last_dir: isize,
    /// Consecutive undecodable files skipped; stops runaway skipping.
    skips: usize,
}

impl App {
    pub fn new(start_path: PathBuf, proxy: EventLoopProxy<()>) -> Self {
        Self {
            start_path,
            window: None,
            renderer: None,
            viewer: Viewer::new((1.0, 1.0)),
            navigator: None,
            loader: Loader::new(5, move || {
                // Wakes the event loop; `user_event` then drains the loader.
                let _ = proxy.send_event(());
            }),
            current: None,
            cursor: (0.0, 0.0),
            panning: false,
            last_frame: Instant::now(),
            last_dir: 1,
            skips: 0,
        }
    }

    fn redraw(&self) {
        if let Some(w) = &self.window {
            w.request_redraw();
        }
    }

    fn show(&mut self, image: Arc<Decoded>) {
        if let Some(r) = &mut self.renderer {
            r.set_image(&image);
        }
        self.viewer.set_image((image.width as f32, image.height as f32));
        self.current = Some(image);
        self.preload_neighbors();
        self.redraw();
    }

    fn navigate(&mut self, delta: isize) {
        self.last_dir = delta.signum();
        self.skips = 0;
        let Some(nav) = &mut self.navigator else { return };
        let Some(path) = nav.step(delta).map(|p| p.to_path_buf()) else { return };
        if let Some(hit) = self.loader.request(path) {
            self.show(hit);
        }
        // Otherwise `user_event` shows it once the decode finishes.
    }

    /// Pick up finished background decodes. Show one if it's the file the
    /// navigator is currently pointing at.
    fn drain_loader(&mut self) {
        let polled = self.loader.poll();
        for decoded in polled.ready {
            let wanted = self
                .navigator
                .as_ref()
                .and_then(|n| n.current())
                .map(|p| p == decoded.path)
                .unwrap_or(false);
            if wanted {
                self.skips = 0;
                self.show(decoded);
            }
        }
        for path in polled.failed {
            let is_current = self
                .navigator
                .as_ref()
                .and_then(|n| n.current())
                .map(|p| p == path)
                .unwrap_or(false);
            if is_current {
                self.skip_broken();
            }
        }
    }

    /// The file we are pointing at could not be decoded: move on in the
    /// direction the user was going.
    fn skip_broken(&mut self) {
        let Some(nav) = &mut self.navigator else { return };
        self.skips += 1;
        if self.skips >= nav.len() {
            log::error!("no viewable images in this folder");
            return;
        }
        let Some(path) = nav.step(self.last_dir).map(|p| p.to_path_buf()) else { return };
        if let Some(hit) = self.loader.request(path) {
            self.skips = 0;
            self.show(hit);
        }
    }

    fn preload_neighbors(&mut self) {
        let Some(nav) = &self.navigator else { return };
        for d in [1, -1] {
            if let Some(p) = nav.peek(d) {
                self.loader.preload(p.to_path_buf());
            }
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attrs = Window::default_attributes()
            .with_title("Pix")
            .with_window_icon(winit::window::Icon::from_rgba(crate::icon::rgba(64), 64, 64).ok())
            .with_decorations(false)
            .with_transparent(true)
            // Stay hidden until the first dimmed frame is presented; otherwise
            // the OS shows a blank (white) window while the GPU is starting.
            .with_visible(false)
            .with_fullscreen(Some(Fullscreen::Borderless(None)));
        let window = Arc::new(el.create_window(attrs).expect("create window"));

        let size = window.inner_size();
        self.viewer.resize((size.width as f32, size.height as f32));
        let mut renderer = Renderer::new(window.clone()).expect("init renderer");
        // Present the dim backdrop first, then reveal the window.
        renderer.draw(self.viewer.uniform(), false);
        window.set_visible(true);
        self.renderer = Some(renderer);
        self.window = Some(window);

        match Navigator::open(&self.start_path) {
            Ok(nav) => {
                if let Some(p) = nav.current() {
                    self.loader.request(p.to_path_buf());
                }
                self.navigator = Some(nav);
            }
            Err(e) => log::error!("cannot open {}: {e}", self.start_path.display()),
        }
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => el.exit(),

            WindowEvent::Resized(size) => {
                if let Some(r) = &mut self.renderer {
                    r.resize(size.width, size.height);
                }
                self.viewer.resize((size.width as f32, size.height as f32));
                // Fullscreen may report its real size after the first image
                // is shown; refit so it is centered for the final size.
                if self.current.is_some() {
                    self.viewer.fit();
                }
                self.redraw();
            }

            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                match event.logical_key {
                    Key::Named(NamedKey::Escape) => el.exit(),
                    Key::Named(NamedKey::ArrowRight) => self.navigate(1),
                    Key::Named(NamedKey::ArrowLeft) => self.navigate(-1),
                    Key::Character(c) => match c.as_str() {
                        "0" => {
                            self.viewer.fit();
                            self.redraw();
                        }
                        "1" => {
                            self.viewer.actual_size();
                            self.redraw();
                        }
                        _ => {}
                    },
                    _ => {}
                }
            }

            WindowEvent::MouseWheel { delta, .. } => {
                let notches = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y,
                    MouseScrollDelta::PixelDelta(p) => p.y as f32 / 40.0,
                };
                self.viewer.zoom_at(notches, self.cursor);
                self.redraw();
            }

            WindowEvent::MouseInput { state, button, .. } => match (button, state) {
                (MouseButton::Left, s) => self.panning = s == ElementState::Pressed,
                (MouseButton::Right, ElementState::Pressed) => {
                    if let Some(img) = &self.current {
                        clipboard::copy_image(img.width, img.height, img.rgba.clone());
                    }
                }
                _ => {}
            },

            WindowEvent::CursorMoved { position: PhysicalPosition { x, y }, .. } => {
                let new = (x as f32, y as f32);
                if self.panning {
                    self.viewer.pan_by(new.0 - self.cursor.0, new.1 - self.cursor.1);
                    self.redraw();
                }
                self.cursor = new;
            }

            WindowEvent::RedrawRequested => {
                let now = Instant::now();
                let dt = (now - self.last_frame).as_secs_f32().min(0.05);
                self.last_frame = now;

                self.viewer.tick(dt);
                if let Some(r) = &mut self.renderer {
                    r.draw(self.viewer.uniform(), self.viewer.use_nearest());
                }
                if self.viewer.is_animating() {
                    self.redraw();
                }
            }

            _ => {}
        }
    }

    /// Sent by the decoder thread through the `EventLoopProxy`.
    fn user_event(&mut self, _el: &ActiveEventLoop, _event: ()) {
        self.drain_loader();
    }
}
