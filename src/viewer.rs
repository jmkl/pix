//! Pure view state: zoom/pan math and smoothing. No GPU or window code here,
//! so it can be unit-tested.

const MIN_ZOOM_FACTOR: f32 = 0.1; // relative to fit-to-screen
const MAX_ZOOM_ABS: f32 = 64.0; // absolute pixels-per-pixel
const ZOOM_STEP: f32 = 1.15; // per wheel notch
const ZOOM_SMOOTHING: f32 = 18.0; // higher = snappier

#[derive(Debug, Clone, Copy)]
pub struct Viewer {
    /// Window size in physical pixels.
    pub window: (f32, f32),
    /// Image size in pixels.
    pub image: (f32, f32),
    /// Current (animated) zoom, in screen pixels per image pixel.
    pub zoom: f32,
    /// Where `zoom` is heading.
    pub target_zoom: f32,
    /// Image center offset from window center, in screen pixels.
    pub offset: (f32, f32),
    /// Cursor position the zoom is anchored on (screen pixels, window space).
    anchor: (f32, f32),
}

impl Viewer {
    pub fn new(window: (f32, f32)) -> Self {
        Self {
            window,
            image: (1.0, 1.0),
            zoom: 1.0,
            target_zoom: 1.0,
            offset: (0.0, 0.0),
            anchor: (window.0 * 0.5, window.1 * 0.5),
        }
    }

    pub fn resize(&mut self, window: (f32, f32)) {
        self.window = window;
    }

    /// Show a new image, fitted to the window and centered.
    pub fn set_image(&mut self, size: (f32, f32)) {
        self.image = size;
        self.fit();
    }

    pub fn fit_zoom(&self) -> f32 {
        let zx = self.window.0 / self.image.0;
        let zy = self.window.1 / self.image.1;
        // Never upscale past 100% on fit, and leave a small margin.
        (zx.min(zy) * 0.92).min(1.0).max(f32::EPSILON)
    }

    pub fn fit(&mut self) {
        let z = self.fit_zoom();
        self.zoom = z;
        self.target_zoom = z;
        self.offset = (0.0, 0.0);
    }

    /// `notches` > 0 zooms in. `cursor` is in window pixels.
    pub fn zoom_at(&mut self, notches: f32, cursor: (f32, f32)) {
        self.anchor = cursor;
        let min = self.fit_zoom() * MIN_ZOOM_FACTOR;
        self.target_zoom =
            (self.target_zoom * ZOOM_STEP.powf(notches)).clamp(min, MAX_ZOOM_ABS);
    }

    /// Animate to 100% (one image pixel per screen pixel), around the window center.
    pub fn actual_size(&mut self) {
        self.anchor = (self.window.0 * 0.5, self.window.1 * 0.5);
        let min = self.fit_zoom() * MIN_ZOOM_FACTOR;
        self.target_zoom = 1.0_f32.clamp(min, MAX_ZOOM_ABS);
    }

    pub fn pan_by(&mut self, dx: f32, dy: f32) {
        self.offset.0 += dx;
        self.offset.1 += dy;
        self.clamp_offset();
    }

    pub fn is_animating(&self) -> bool {
        (self.zoom - self.target_zoom).abs() > self.target_zoom * 1e-3
    }

    /// Advance the zoom animation by `dt` seconds. Frame-rate independent.
    pub fn tick(&mut self, dt: f32) {
        if !self.is_animating() {
            self.zoom = self.target_zoom;
            return;
        }
        let old = self.zoom;
        let t = 1.0 - (-ZOOM_SMOOTHING * dt).exp();
        self.zoom += (self.target_zoom - self.zoom) * t;

        // Keep the image point under the cursor fixed while zoom changes.
        let ratio = self.zoom / old;
        let cx = self.anchor.0 - self.window.0 * 0.5;
        let cy = self.anchor.1 - self.window.1 * 0.5;
        self.offset.0 = cx + (self.offset.0 - cx) * ratio;
        self.offset.1 = cy + (self.offset.1 - cy) * ratio;
        self.clamp_offset();
    }

    /// Keep at least a sliver of the image on screen.
    fn clamp_offset(&mut self) {
        let half_w = self.image.0 * self.zoom * 0.5;
        let half_h = self.image.1 * self.zoom * 0.5;
        let limit_x = self.window.0 * 0.5 + half_w - 64.0;
        let limit_y = self.window.1 * 0.5 + half_h - 64.0;
        self.offset.0 = self.offset.0.clamp(-limit_x, limit_x);
        self.offset.1 = self.offset.1.clamp(-limit_y, limit_y);
    }

    /// Scale and offset in normalized device coordinates for the quad shader.
    /// Returns `(scale_x, scale_y, offset_x, offset_y)`.
    pub fn uniform(&self) -> [f32; 4] {
        let sx = self.image.0 * self.zoom / self.window.0;
        let sy = self.image.1 * self.zoom / self.window.1;
        let ox = self.offset.0 * 2.0 / self.window.0;
        let oy = -self.offset.1 * 2.0 / self.window.1; // screen y is down, NDC y is up
        [sx, sy, ox, oy]
    }

    /// Use nearest-neighbor sampling when zoomed far in.
    pub fn use_nearest(&self) -> bool {
        self.zoom > 3.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_centers_and_never_upscales() {
        let mut v = Viewer::new((1920.0, 1080.0));
        v.set_image((100.0, 100.0));
        assert_eq!(v.zoom, 1.0);
        assert_eq!(v.offset, (0.0, 0.0));
    }

    #[test]
    fn zoom_converges() {
        let mut v = Viewer::new((1920.0, 1080.0));
        v.set_image((800.0, 600.0));
        v.zoom_at(5.0, (960.0, 540.0));
        for _ in 0..240 {
            v.tick(1.0 / 60.0);
        }
        assert!(!v.is_animating());
        assert!((v.zoom - v.target_zoom).abs() < 1e-3);
    }
}
