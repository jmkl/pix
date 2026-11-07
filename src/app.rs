//! winit application: translates input into viewer/navigator actions and
//! schedules redraws only when something changes.

use std::path::PathBuf;
use std::sync::Arc;
use std::thread::JoinHandle;
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
    /// wgpu instance being created on a background thread (see `main`).
    instance: Option<JoinHandle<wgpu::Instance>>,
    viewer: Viewer,
    navigator: Option<Navigator>,
    loader: Loader,
    current: Option<Arc<Decoded>>,
    cursor: (f32, f32),
    panning: bool,
    last_frame: Instant,
    /// True while consecutive animation frames are being drawn. After idle the
    /// first frame uses a nominal dt instead of the long gap since the last one.
    frame_chain: bool,
    /// Time of the last drag event, for fling velocity.
    last_move: Instant,
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
            instance: None,
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
            frame_chain: false,
            last_move: Instant::now(),
            last_dir: 1,
            skips: 0,
        }
    }

    pub fn set_instance(&mut self, handle: JoinHandle<wgpu::Instance>) {
        self.instance = Some(handle);
    }

    /// Start decoding the first image immediately. Called before the event
    /// loop runs, so decoding overlaps with window and GPU startup. The folder
    /// scan is deliberately NOT done here: it is only needed for arrow-key
    /// navigation, so it runs after the first frame (see `open_navigator`).
    pub fn prefetch(&mut self) {
        // Same form Navigator uses, so the decoded path matches later lookups.
        self.start_path = self
            .start_path
            .canonicalize()
            .unwrap_or_else(|_| self.start_path.clone());
        self.loader.request(self.start_path.clone());
    }

    /// Scan the image's folder (natural sort) and queue neighbor preloads.
    /// Idempotent; kept off the startup critical path.
    fn open_navigator(&mut self) {
        if self.navigator.is_some() {
            return;
        }
        let t0 = Instant::now();
        match Navigator::open(&self.start_path) {
            Ok(nav) => self.navigator = Some(nav),
            Err(e) => log::error!("cannot open {}: {e}", self.start_path.display()),
        }
        log::info!("[{:>4} ms] folder scan in {:?}", crate::ms_since_start(), t0.elapsed());
    }

    fn redraw(&self) {
        if let Some(w) = &self.window {
            w.request_redraw();
        }
    }

    fn show(&mut self, image: Arc<Decoded>) {
        let Some(r) = &mut self.renderer else {
            // Decoded before the GPU is ready: `resumed` shows it.
            self.current = Some(image);
            return;
        };
        let t0 = Instant::now();
        r.set_image(&image);
        log::info!(
            "[{:>4} ms] upload {}x{} in {:?}",
            crate::ms_since_start(),
            image.width,
            image.height,
            t0.elapsed()
        );
        self.viewer.set_image((image.width as f32, image.height as f32));
        self.current = Some(image);
        self.preload_neighbors();
        self.redraw();
    }

    fn navigate(&mut self, delta: isize) {
        self.last_dir = delta.signum();
        self.skips = 0;
        self.open_navigator();
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
            // Before the folder scan has run, the only image we want is the
            // one we were launched with.
            let wanted = match &self.navigator {
                Some(n) => n.current().map(|p| p == decoded.path).unwrap_or(false),
                None => decoded.path == self.start_path,
            };
            if wanted {
                self.skips = 0;
                self.show(decoded);
            }
        }
        for path in polled.failed {
            let is_current = match &self.navigator {
                Some(n) => n.current().map(|p| p == path).unwrap_or(false),
                None => path == self.start_path,
            };
            if is_current {
                self.skip_broken();
            }
        }
    }

    /// The file we are pointing at could not be decoded: move on in the
    /// direction the user was going.
    fn skip_broken(&mut self) {
        self.open_navigator();
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
        let t0 = Instant::now();
        let window = Arc::new(el.create_window(attrs).expect("create window"));
        log::info!("[{:>4} ms] window created in {:?}", crate::ms_since_start(), t0.elapsed());

        let size = window.inner_size();
        self.viewer.resize((size.width as f32, size.height as f32));
        let t0 = Instant::now();
        let instance = match self.instance.take() {
            Some(h) => h.join().unwrap_or_else(|_| crate::renderer::create_instance()),
            None => crate::renderer::create_instance(),
        };
        log::info!("[{:>4} ms] wgpu instance ready", crate::ms_since_start());
        let renderer = Renderer::new(window.clone(), instance).expect("init renderer");
        log::info!("renderer ready in {:?}", t0.elapsed());
        log::info!("[{:>4} ms] renderer ready", crate::ms_since_start());
        self.renderer = Some(renderer);
        self.window = Some(window.clone());

        // Pick up anything decoded while the GPU was starting; the wake event
        // can't be delivered before the loop runs, so poll the channel here.
        // This uploads the image (once) so the first frame already has it.
        self.drain_loader();
        // Present the dim backdrop (and image, if any), then reveal the window.
        if let Some(r) = &mut self.renderer {
            let t0 = Instant::now();
            r.draw(self.viewer.uniform(), false);
            log::info!("[{:>4} ms] first frame in {:?}", crate::ms_since_start(), t0.elapsed());
        }
        window.set_visible(true);
        log::info!("[{:>4} ms] window visible", crate::ms_since_start());

        // Everything below is off the critical path: the image is on screen.
        self.open_navigator();
        self.preload_neighbors();
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
                            self.viewer.fit_animated();
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
                (MouseButton::Left, ElementState::Pressed) => {
                    // Clicking the dimmed backdrop outside the image quits, like Esc.
                    if self.current.is_some() && !self.viewer.image_contains(self.cursor) {
                        el.exit();
                        return;
                    }
                    self.panning = true;
                    self.last_move = Instant::now();
                    self.viewer.begin_drag();
                }
                (MouseButton::Left, ElementState::Released) => {
                    if self.panning {
                        self.panning = false;
                        self.viewer.end_drag(self.last_move.elapsed().as_secs_f32());
                        self.redraw();
                    }
                }
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
                    let now = Instant::now();
                    let dt = (now - self.last_move).as_secs_f32();
                    self.last_move = now;
                    self.viewer.drag_by(new.0 - self.cursor.0, new.1 - self.cursor.1, dt);
                    self.redraw();
                }
                self.cursor = new;
            }

            WindowEvent::RedrawRequested => {
                let now = Instant::now();
                let dt = if self.frame_chain {
                    (now - self.last_frame).as_secs_f32().min(0.05)
                } else {
                    1.0 / 60.0
                };
                self.last_frame = now;

                self.viewer.tick(dt);
                if let Some(r) = &mut self.renderer {
                    r.draw(self.viewer.uniform(), self.viewer.use_nearest());
                }
                self.frame_chain = self.viewer.is_animating();
                if self.frame_chain {
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
