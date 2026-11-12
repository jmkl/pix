//! Minimal, fast image viewer.
//!
//!   * Left mouse drag   -> move the (title-bar-less) window; once the image is
//!                          larger than the window, or in fullscreen -> pan the image
//!   * Mouse wheel       -> zoom (centered on the cursor)
//!   * Right click       -> toggle fullscreen
//!   * Esc               -> quit
//!   * Zoom (windowed)   -> also resizes the window around its center, until it
//!                          reaches the monitor size; then the image zooms inside it
//!   * Top-right button  -> copy the image to the clipboard
//!   * Window is always on top
//!
//! Usage: minimal-image-viewer <image-file>
//!
//! Speed design (same goals as rust-image-viewer's ARCHITECTURE.md §1):
//!   * Decoding starts on a worker thread *before* the window/GL context exists,
//!     so window startup and decode run in parallel.
//!   * JPEG/PNG/BMP use zune-image (SIMD) over a memory-mapped file; everything
//!     else falls back to the `image` crate.
//!   * The GPU-ready pixel buffer is built on the worker, not the UI thread.
//!   * One resize boundary (`downscale_if_needed`) with fast_image_resize.
//!   * Nothing repaints unless input or a finished decode asks for it (0% idle CPU).

#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]
mod icon;
use eframe::egui::{self, vec2, Color32, PointerButton, Rect, Sense, Stroke, Vec2};
use fast_image_resize as fir;
use image::{metadata::Orientation, DynamicImage, ImageDecoder, ImageFormat, ImageReader};
use memmap2::Mmap;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
use zune_core::{colorspace::ColorSpace, options::DecoderOptions};
use zune_image::image::Image as ZuneImage;

/// Largest texture side uploaded to the GPU (bounds VRAM to ~256 MB). Bigger
/// images are downscaled for display only; layout/zoom use the original size.
const MAX_TEXTURE_SIDE: u32 = 8192;
/// Refuse absurd images (decompression bombs) before allocating.
const MAX_PIXELS: u64 = 400_000_000;
const MIN_ZOOM: f32 = 0.02;
const MAX_ZOOM: f32 = 64.0;
/// Zoom speed: factor = exp(scroll_points * ZOOM_SPEED).
const ZOOM_SPEED: f32 = 0.0055;
const BG: Color32 = Color32::from_gray(18);

/// Set once the window exists so the decode thread can wake the UI.
static CTX: OnceLock<egui::Context> = OnceLock::new();

// ───────────────────────────── decoding ──────────────────────────────

struct Decoded {
    /// GPU-ready pixels (possibly downscaled to MAX_TEXTURE_SIDE).
    color: egui::ColorImage,
    /// Original size in pixels after EXIF rotation.
    size: (u32, u32),
}

fn swaps_axes(o: Orientation) -> bool {
    matches!(
        o,
        Orientation::Rotate90
            | Orientation::Rotate270
            | Orientation::Rotate90FlipH
            | Orientation::Rotate270FlipH
    )
}

/// Header-only read: (width, height, orientation). Microseconds, no pixel decode.
fn probe(bytes: &[u8]) -> Option<(u32, u32, Orientation, Option<ImageFormat>)> {
    let reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    let format = reader.format();
    let mut dec = reader.into_decoder().ok()?;
    let (w, h) = dec.dimensions();
    let o = dec.orientation().unwrap_or(Orientation::NoTransforms);
    Some((w, h, o, format))
}

/// Fast path: zune-image. Returns (w, h, RGBA bytes).
fn decode_zune(bytes: &[u8], jpeg: bool, w: u32, h: u32) -> Result<(u32, u32, Vec<u8>), String> {
    let mut options = DecoderOptions::new_fast()
        .png_set_decode_animated(false)
        .set_max_width(w as usize)
        .set_max_height(h as usize);
    if jpeg {
        options = options.jpeg_set_out_colorspace(ColorSpace::RGBA);
    }
    let mut img = ZuneImage::read(Cursor::new(bytes), options).map_err(|e| e.to_string())?;
    img.convert_color(ColorSpace::RGBA)
        .map_err(|e| e.to_string())?;
    let (w, h) = img.dimensions();
    let pixels = img
        .flatten_to_u8()
        .into_iter()
        .next()
        .ok_or("decoder returned no pixels")?;
    if pixels.len() != w * h * 4 {
        return Err("pixel buffer size mismatch".into());
    }
    Ok((w as u32, h as u32, pixels))
}

/// Fallback path: the `image` crate. Returns (w, h, RGBA bytes, has_alpha), orientation applied.
fn decode_image_crate(bytes: &[u8]) -> Result<(u32, u32, Vec<u8>, bool), String> {
    let mut dec = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| e.to_string())?
        .into_decoder()
        .map_err(|e| e.to_string())?;
    let o = dec.orientation().unwrap_or(Orientation::NoTransforms);
    let mut img = DynamicImage::from_decoder(dec).map_err(|e| e.to_string())?;
    img.apply_orientation(o);
    let has_alpha = img.color().has_alpha();
    let rgba = img.into_rgba8();
    Ok((rgba.width(), rgba.height(), rgba.into_raw(), has_alpha))
}

/// The single resize boundary: shrink only if the image exceeds the GPU limit.
fn downscale_if_needed(w: u32, h: u32, rgba: Vec<u8>) -> Result<(u32, u32, Vec<u8>), String> {
    if w <= MAX_TEXTURE_SIDE && h <= MAX_TEXTURE_SIDE {
        return Ok((w, h, rgba));
    }
    let scale = (MAX_TEXTURE_SIDE as f64 / w as f64).min(MAX_TEXTURE_SIDE as f64 / h as f64);
    let nw = ((w as f64 * scale).round() as u32).max(1);
    let nh = ((h as f64 * scale).round() as u32).max(1);

    let src =
        fir::images::ImageRef::new(w, h, &rgba, fir::PixelType::U8x4).map_err(|e| e.to_string())?;
    let mut dst = fir::images::Image::new(nw, nh, fir::PixelType::U8x4);
    let options = fir::ResizeOptions::new()
        .resize_alg(fir::ResizeAlg::Convolution(fir::FilterType::Bilinear));
    fir::Resizer::new()
        .resize(&src, &mut dst, Some(&options))
        .map_err(|e| e.to_string())?;
    Ok((nw, nh, dst.into_vec()))
}

/// Full-resolution, EXIF-oriented RGBA. Returns (w, h, pixels, has_alpha).
fn decode_rgba(path: &Path) -> Result<(u32, u32, Vec<u8>, bool), String> {
    let file = std::fs::File::open(path).map_err(|e| format!("Failed to open file: {e}"))?;
    // SAFETY: the mapping is read-only and owned for the duration of the decode.
    let mmap = unsafe { Mmap::map(&file) }.map_err(|e| format!("Failed to map file: {e}"))?;

    let (w, h, orientation, format) = probe(&mmap).ok_or("Unsupported or corrupt image")?;
    if (w as u64) * (h as u64) > MAX_PIXELS {
        return Err(format!("Image too large ({w}x{h})"));
    }

    // Fast path for the common formats; any failure falls through to `image`.
    let zune = match format {
        Some(ImageFormat::Jpeg) => decode_zune(&mmap, true, w, h).ok().map(|r| (r, false)),
        Some(ImageFormat::Png) | Some(ImageFormat::Bmp) => {
            decode_zune(&mmap, false, w, h).ok().map(|r| (r, true))
        }
        _ => None,
    };

    match zune {
        Some(((w, h, mut px), has_alpha)) => {
            let (mut w, mut h) = (w, h);
            if !matches!(orientation, Orientation::NoTransforms) {
                let buf = image::RgbaImage::from_raw(w, h, px).ok_or("bad pixel buffer")?;
                let mut dynimg = DynamicImage::ImageRgba8(buf);
                dynimg.apply_orientation(orientation);
                let out = dynimg.into_rgba8();
                (w, h) = (out.width(), out.height());
                px = out.into_raw();
            }
            Ok((w, h, px, has_alpha))
        }
        None => decode_image_crate(&mmap),
    }
}

/// Display decode: full decode, then the single downscale boundary, then the
/// GPU-ready buffer (all on the worker thread).
fn decode(path: &Path) -> Result<Decoded, String> {
    let (w, h, rgba, has_alpha) = decode_rgba(path)?;
    let size = (w, h);
    let (tw, th, rgba) = downscale_if_needed(w, h, rgba)?;
    let dims = [tw as usize, th as usize];
    // Opaque images skip the per-pixel premultiply.
    let color = if has_alpha {
        egui::ColorImage::from_rgba_unmultiplied(dims, &rgba)
    } else {
        egui::ColorImage::from_rgba_premultiplied(dims, &rgba)
    };
    Ok(Decoded { color, size })
}

/// Put the full-resolution image on the clipboard. Re-decodes from disk (on a
/// worker thread) so no full-size copy has to stay in RAM while viewing.
fn copy_to_clipboard(path: &Path) -> Result<(), String> {
    let (w, h, rgba, _) = decode_rgba(path)?;
    arboard::Clipboard::new()
        .map_err(|e| e.to_string())?
        .set_image(arboard::ImageData {
            width: w as usize,
            height: h as usize,
            bytes: std::borrow::Cow::Owned(rgba),
        })
        .map_err(|e| e.to_string())
}

// ───────────────────────────── viewer ────────────────────────────────

/// What the current left-button drag is doing, latched at press time.
#[derive(Clone, Copy)]
enum DragMode {
    None,
    /// Pan the image inside the window.
    Pan,
    /// Move the window. Positions are physical pixels so a DPI change mid-drag
    /// (monitor with another scale) cannot leak into the delta.
    Window {
        outer_px: egui::Pos2,
        cursor_px: egui::Pos2,
    },
}

/// Global cursor position in physical screen pixels. Sampling it directly (rather
/// than egui's window-relative pointer) keeps the window glued to the cursor even
/// while the window itself is moving. It also avoids the native `StartDrag`,
/// which triggers Windows 11's top-edge snap/maximize on a borderless window.
#[cfg(windows)]
fn global_cursor_px() -> Option<egui::Pos2> {
    // One Win32 call, declared by hand so no Windows crate is needed.
    #[repr(C)]
    struct Point {
        x: i32,
        y: i32,
    }
    #[link(name = "user32")]
    extern "system" {
        fn GetCursorPos(point: *mut Point) -> i32;
    }

    let mut p = Point { x: 0, y: 0 };
    // SAFETY: `p` is a valid, writable POINT for the duration of the call.
    if unsafe { GetCursorPos(&mut p) } != 0 {
        Some(egui::pos2(p.x as f32, p.y as f32))
    } else {
        None
    }
}

#[cfg(not(windows))]
fn global_cursor_px() -> Option<egui::Pos2> {
    None
}

const COPY_IDLE: u8 = 0;
const COPY_BUSY: u8 = 1;
const COPY_OK: u8 = 2;
const COPY_ERR: u8 = 3;
const COPY_FEEDBACK: Duration = Duration::from_millis(1500);
const BTN_SIZE: f32 = 30.0;
const BTN_MARGIN: f32 = 10.0;
/// Smallest window the zoom-resize will shrink to.
const MIN_WINDOW: [f32; 2] = [200.0, 150.0];

struct ViewerApp {
    path: Option<PathBuf>,
    rx: Option<Receiver<Result<Decoded, String>>>,
    texture: Option<egui::TextureHandle>,
    /// Original image size in pixels.
    size: (u32, u32),
    error: Option<String>,
    /// Display scale relative to 1 image pixel == 1 screen pixel.
    zoom: f32,
    /// Offset of the image center from the viewport center, in points.
    offset: Vec2,
    /// While true the image is re-fitted to the window every frame
    /// (initial state, and after toggling fullscreen).
    fitted: bool,
    drag: DragMode,
    copy_state: Arc<AtomicU8>,
    copy_done_at: Option<Instant>,
}

impl ViewerApp {
    fn new(
        cc: &eframe::CreationContext<'_>,
        path: Option<PathBuf>,
        rx: Option<Receiver<Result<Decoded, String>>>,
        error: Option<String>,
    ) -> Self {
        // Publish the context first so a decode finishing from now on can wake us.
        let _ = CTX.set(cc.egui_ctx.clone());
        Self {
            path,
            rx,
            texture: None,
            size: (1, 1),
            error,
            zoom: 1.0,
            offset: Vec2::ZERO,
            fitted: true,
            drag: DragMode::None,
            copy_state: Arc::new(AtomicU8::new(COPY_IDLE)),
            copy_done_at: None,
        }
    }

    fn poll_decode(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.rx else { return };
        match rx.try_recv() {
            Ok(Ok(d)) => {
                self.texture =
                    Some(ctx.load_texture("image", d.color, egui::TextureOptions::LINEAR));
                self.size = d.size;
                self.rx = None;
            }
            Ok(Err(e)) => {
                self.error = Some(e);
                self.rx = None;
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                self.error = Some("Decoder thread stopped unexpectedly".into());
                self.rx = None;
            }
        }
    }

    fn start_copy(&self) {
        let Some(path) = self.path.clone() else {
            return;
        };
        self.copy_state.store(COPY_BUSY, Ordering::Relaxed);
        let state = self.copy_state.clone();
        std::thread::spawn(move || {
            let ok = copy_to_clipboard(&path).is_ok();
            state.store(if ok { COPY_OK } else { COPY_ERR }, Ordering::Relaxed);
            if let Some(ctx) = CTX.get() {
                ctx.request_repaint();
            }
        });
    }
}

/// Resize the window's inner size to `target`, keeping its center fixed.
fn resize_window_centered(ctx: &egui::Context, target: Vec2) {
    let (inner, outer) = ctx.input(|i| (i.viewport().inner_rect, i.viewport().outer_rect));
    let (Some(inner), Some(outer)) = (inner, outer) else {
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(target));
        return;
    };
    if (inner.size() - target).length() <= 0.5 {
        return;
    }
    let inner_to_outer = inner.min - outer.min;
    let outer_min = inner.center() - target * 0.5 - inner_to_outer;
    ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(outer_min));
    ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(target));
}

fn outline(p: &egui::Painter, r: Rect, s: Stroke) {
    let [a, b, c, d] = [
        r.left_top(),
        r.right_top(),
        r.right_bottom(),
        r.left_bottom(),
    ];
    for (x, y) in [(a, b), (b, c), (c, d), (d, a)] {
        p.line_segment([x, y], s);
    }
}

/// The copy button, drawn from plain shapes (no icon font needed).
fn draw_copy_button(p: &egui::Painter, rect: Rect, hovered: bool, state: u8) {
    let bg = Color32::from_black_alpha(if hovered { 210 } else { 140 });
    p.rect_filled(rect, 6.0, bg);
    let fg = Color32::from_gray(235);
    let stroke = Stroke::new(1.6, fg);
    let c = rect.center();
    match state {
        COPY_OK => {
            p.line_segment([c + vec2(-6.0, 0.0), c + vec2(-2.0, 5.0)], stroke);
            p.line_segment([c + vec2(-2.0, 5.0), c + vec2(7.0, -5.0)], stroke);
        }
        COPY_ERR => {
            p.line_segment([c + vec2(-5.0, -5.0), c + vec2(5.0, 5.0)], stroke);
            p.line_segment([c + vec2(-5.0, 5.0), c + vec2(5.0, -5.0)], stroke);
        }
        COPY_BUSY => {
            for dx in [-6.0, 0.0, 6.0] {
                p.circle_filled(c + vec2(dx, 0.0), 1.8, fg);
            }
        }
        _ => {
            // Two overlapping pages: only the back page's top/left edges show.
            let back = Rect::from_center_size(c + vec2(-2.5, -2.5), vec2(10.0, 10.0));
            let front = Rect::from_center_size(c + vec2(2.5, 2.5), vec2(10.0, 10.0));
            p.line_segment([back.left_bottom(), back.left_top()], stroke);
            p.line_segment([back.left_top(), back.right_top()], stroke);
            outline(p, front, stroke);
        }
    }
}

impl eframe::App for ViewerApp {
    // fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
    //     BG.to_normalized_gamma_f32()
    // }

    // eframe 0.36+: `ui` replaces `update`. The root `Ui` has no margin or
    // background, so no CentralPanel is needed.
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.poll_decode(&ctx);

        // Esc: quit.
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }

        let fullscreen = ctx.input(|i| i.viewport().fullscreen.unwrap_or(false));
        let view = ui.max_rect();
        let response = ui.allocate_rect(view, Sense::click_and_drag());

        // Right click: toggle fullscreen (works while loading, too).
        if response.secondary_clicked() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(!fullscreen));
            self.fitted = true;
        }

        let Some(texture) = self.texture.clone() else {
            if let Some(msg) = &self.error {
                ui.painter().text(
                    view.center(),
                    egui::Align2::CENTER_CENTER,
                    msg,
                    egui::FontId::proportional(16.0),
                    Color32::LIGHT_GRAY,
                );
            }
            return;
        };

        // Copy button (top right). Allocated after the full-view response, so it
        // wins hit-testing: clicking it never starts a window drag.
        let btn_rect = Rect::from_min_size(
            egui::pos2(view.max.x - BTN_MARGIN - BTN_SIZE, view.min.y + BTN_MARGIN),
            vec2(BTN_SIZE, BTN_SIZE),
        );
        let btn = ui.allocate_rect(btn_rect, Sense::click());
        let mut copy_state = self.copy_state.load(Ordering::Relaxed);
        if copy_state == COPY_OK || copy_state == COPY_ERR {
            let since = self.copy_done_at.get_or_insert_with(Instant::now).elapsed();
            if since >= COPY_FEEDBACK {
                self.copy_state.store(COPY_IDLE, Ordering::Relaxed);
                self.copy_done_at = None;
                copy_state = COPY_IDLE;
            } else {
                ctx.request_repaint_after(COPY_FEEDBACK - since);
            }
        }
        if btn.clicked() && copy_state != COPY_BUSY {
            self.start_copy();
            copy_state = COPY_BUSY;
        }
        if btn.hovered() {
            ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
        }

        // Image size in points at zoom == 1 (1 image px == 1 screen px).
        let ppp = ctx.pixels_per_point();
        let base = Vec2::new(self.size.0 as f32, self.size.1 as f32) / ppp;
        let fit = (view.width() / base.x).min(view.height() / base.y);

        if self.fitted {
            self.zoom = fit.clamp(MIN_ZOOM, MAX_ZOOM);
            self.offset = Vec2::ZERO;
        }

        // Left drag: move the window, or pan once the image outgrows it.
        if response.drag_started_by(PointerButton::Primary) {
            let shown = base * self.zoom;
            let exceeds_window = shown.x > view.width() + 1.0 || shown.y > view.height() + 1.0;
            self.drag = if fullscreen || exceeds_window {
                DragMode::Pan
            } else if let Some(cursor_px) = global_cursor_px() {
                let outer_px = ctx
                    .input(|i| i.viewport().outer_rect)
                    .map(|r| r.min * ppp)
                    .unwrap_or(egui::Pos2::ZERO);
                DragMode::Window {
                    outer_px,
                    cursor_px,
                }
            } else {
                // No global cursor query on this platform: let the OS drive the move.
                ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
                DragMode::None
            };
        }
        if response.dragged_by(PointerButton::Primary) {
            match self.drag {
                DragMode::Pan => {
                    self.offset += response.drag_delta();
                    self.fitted = false;
                }
                DragMode::Window {
                    outer_px,
                    cursor_px,
                } => {
                    if let Some(now) = global_cursor_px() {
                        let pos = (outer_px + (now - cursor_px)) / ppp;
                        ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(pos));
                    }
                    // Re-sample the cursor every frame: pointer events stop arriving
                    // once the window lags behind or the cursor leaves it.
                    ctx.request_repaint();
                }
                DragMode::None => {}
            }
            ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
        } else if !response.dragged() {
            self.drag = DragMode::None;
        }

        let raw_scroll_delta = ctx.input(|i| {
            i.events.iter().fold(egui::Vec2::ZERO, |acc, event| {
                if let egui::Event::MouseWheel { delta, unit, .. } = event {
                    // Note: handle unit scaling (Points, Line, Page) if needed
                    acc + *delta
                } else {
                    acc
                }
            })
        });
        println!("{}", raw_scroll_delta);
        // Scroll: zoom.
        let scroll = ctx.input(|i| i.smooth_scroll_delta.y);
        if scroll != 0.0 && response.hovered() {
            if let Some(p) = ctx.input(|i| i.pointer.hover_pos()) {
                let new_zoom = (self.zoom * (scroll * ZOOM_SPEED).exp()).clamp(MIN_ZOOM, MAX_ZOOM);
                let ratio = new_zoom / self.zoom;
                // Keep the image point under the cursor fixed.
                let anchored = |off: Vec2| {
                    let from_center = p - view.center();
                    from_center - (from_center - off) * ratio
                };

                if fullscreen {
                    self.offset = anchored(self.offset);
                } else {
                    // Windowed: the window follows the zoom, growing/shrinking around
                    // its own center, until it reaches the monitor. Beyond that the
                    // window stays monitor-sized (aspect kept) and the image zooms
                    // inside it, anchored at the cursor.
                    let desired = base * new_zoom;
                    let monitor = ctx.input(|i| i.viewport().monitor_size);
                    let mut target = desired;
                    let mut inside = false;
                    if let Some(m) = monitor.filter(|m| m.x > 0.0 && m.y > 0.0) {
                        if desired.x > m.x || desired.y > m.y {
                            target = desired * (m.x / desired.x).min(m.y / desired.y);
                            inside = true;
                        }
                    }
                    let target = vec2(target.x.max(MIN_WINDOW[0]), target.y.max(MIN_WINDOW[1]));
                    self.offset = if inside {
                        anchored(self.offset)
                    } else {
                        Vec2::ZERO
                    };
                    resize_window_centered(&ctx, target);
                }
                self.zoom = new_zoom;
                self.fitted = false;
            }
        }

        let rect = Rect::from_center_size(view.center() + self.offset, base * self.zoom);
        ui.painter().image(
            texture.id(),
            rect,
            Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            Color32::WHITE,
        );

        draw_copy_button(ui.painter(), btn_rect, btn.hovered(), copy_state);
        btn.on_hover_text("Copy image to clipboard");
    }
}

// ────────────────────────────── main ─────────────────────────────────

/// Window size from the header alone, scaled to fit 1280x800 keeping aspect.
fn initial_window_size(path: &Path) -> [f32; 2] {
    let dims = std::fs::File::open(path)
        .ok()
        .and_then(|f| unsafe { Mmap::map(&f) }.ok())
        .and_then(|m| probe(&m))
        .map(|(w, h, o, _)| if swaps_axes(o) { (h, w) } else { (w, h) });
    match dims {
        Some((w, h)) if w > 0 && h > 0 => {
            let (w, h) = (w as f32, h as f32);
            let s = (1280.0 / w).min(800.0 / h).min(1.0);
            [(w * s).max(240.0), (h * s).max(240.0)]
        }
        _ => [800.0, 600.0],
    }
}

fn main() -> eframe::Result<()> {
    let path = std::env::args_os().nth(1).map(PathBuf::from);

    // Start decoding immediately; it overlaps with window + GL creation below.
    let (rx, error) = match &path {
        Some(p) => {
            let (tx, rx) = mpsc::channel();
            let p = p.clone();
            std::thread::spawn(move || {
                let result = decode(&p).map_err(|e| format!("Failed to open {}: {e}", p.display()));
                let _ = tx.send(result);
                if let Some(ctx) = CTX.get() {
                    ctx.request_repaint();
                }
            });
            (Some(rx), None)
        }
        None => (
            None,
            Some("Usage: minimal-image-viewer <image-file>".to_string()),
        ),
    };

    let title = path
        .as_ref()
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Image Viewer".into());
    let inner = path
        .as_deref()
        .map(initial_window_size)
        .unwrap_or([640.0, 360.0]);

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(title.clone())
            .with_decorations(false) // no title bar
            .with_always_on_top()
            .with_transparent(true)
            .with_min_inner_size([200.0, 150.0])
            .with_inner_size(inner),
        centered: true,
        renderer: eframe::Renderer::Glow,
        // 0.37 dithers texture sampling by default; an image viewer must show
        // the pixels as decoded (and skip the extra shader work).
        dithering: false,
        ..Default::default()
    };

    eframe::run_native(
        &title,
        options,
        Box::new(move |cc| Ok(Box::new(ViewerApp::new(cc, path, rx, error)))),
    )
}
