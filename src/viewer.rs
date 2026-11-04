//! Pure view state: zoom/pan math and smoothing. No GPU or window code here,
//! so it can be unit-tested.

const MIN_ZOOM_FACTOR: f32 = 0.1; // relative to fit-to-screen
const MAX_ZOOM_ABS: f32 = 64.0; // absolute pixels-per-pixel
const ZOOM_STEP: f32 = 1.15; // per wheel notch
const ZOOM_SMOOTHING: f32 = 18.0; // higher = snappier
const PAN_FRICTION: f32 = 5.0; // inertia decay rate (1/s); higher = stops sooner
const PAN_STOP_SPEED: f32 = 8.0; // px/s below which a fling stops
const PAN_MAX_SPEED: f32 = 8000.0; // px/s cap on fling speed
const FLING_MAX_IDLE: f32 = 0.08; // s; pausing this long before release cancels the fling
const VELOCITY_WINDOW: f32 = 0.04; // s; smoothing window for drag velocity

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
    /// Pan velocity in screen pixels per second (inertia after a drag).
    velocity: (f32, f32),
    dragging: bool,
    /// When set, `offset` eases toward this (used by animated fit).
    offset_goal: Option<(f32, f32)>,
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
            velocity: (0.0, 0.0),
            dragging: false,
            offset_goal: None,
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
        self.velocity = (0.0, 0.0);
        self.offset_goal = None;
    }

    /// Like `fit`, but eases zoom and position to the fitted state.
    pub fn fit_animated(&mut self) {
        self.target_zoom = self.fit_zoom();
        self.offset_goal = Some((0.0, 0.0));
        self.velocity = (0.0, 0.0);
    }

    /// `notches` > 0 zooms in. `cursor` is in window pixels.
    pub fn zoom_at(&mut self, notches: f32, cursor: (f32, f32)) {
        self.anchor = cursor;
        self.offset_goal = None;
        let min = self.fit_zoom() * MIN_ZOOM_FACTOR;
        self.target_zoom =
            (self.target_zoom * ZOOM_STEP.powf(notches)).clamp(min, MAX_ZOOM_ABS);
    }

    /// Animate to 100% (one image pixel per screen pixel), around the window center.
    pub fn actual_size(&mut self) {
        self.anchor = (self.window.0 * 0.5, self.window.1 * 0.5);
        self.offset_goal = None;
        let min = self.fit_zoom() * MIN_ZOOM_FACTOR;
        self.target_zoom = 1.0_f32.clamp(min, MAX_ZOOM_ABS);
    }

    pub fn pan_by(&mut self, dx: f32, dy: f32) {
        self.offset.0 += dx;
        self.offset.1 += dy;
        self.clamp_offset();
    }

    /// Start a drag: stops any fling or animated fit.
    pub fn begin_drag(&mut self) {
        self.dragging = true;
        self.velocity = (0.0, 0.0);
        self.offset_goal = None;
    }

    /// Pan during a drag. `dt` is the time since the previous drag event, used
    /// to estimate the release velocity.
    pub fn drag_by(&mut self, dx: f32, dy: f32, dt: f32) {
        let before = self.offset;
        self.pan_by(dx, dy);
        let moved = (self.offset.0 - before.0, self.offset.1 - before.1);
        let dt = dt.max(1e-3);
        let a = 1.0 - (-dt / VELOCITY_WINDOW).exp();
        self.velocity.0 += (moved.0 / dt - self.velocity.0) * a;
        self.velocity.1 += (moved.1 / dt - self.velocity.1) * a;
    }

    /// Release the drag. `idle` is the time since the last drag event; if the
    /// pointer rested before release there is no fling.
    pub fn end_drag(&mut self, idle: f32) {
        self.dragging = false;
        if idle > FLING_MAX_IDLE {
            self.velocity = (0.0, 0.0);
            return;
        }
        let speed = self.speed();
        if speed > PAN_MAX_SPEED {
            let k = PAN_MAX_SPEED / speed;
            self.velocity.0 *= k;
            self.velocity.1 *= k;
        }
    }

    fn speed(&self) -> f32 {
        self.velocity.0.hypot(self.velocity.1)
    }

    fn coasting(&self) -> bool {
        !self.dragging && self.speed() > PAN_STOP_SPEED
    }

    fn zoom_pending(&self) -> bool {
        (self.zoom.ln() - self.target_zoom.ln()).abs() > 1e-3
    }

    fn goal_pending(&self) -> bool {
        match self.offset_goal {
            Some(g) => (g.0 - self.offset.0).hypot(g.1 - self.offset.1) > 0.25,
            None => false,
        }
    }

    pub fn is_animating(&self) -> bool {
        self.zoom_pending() || self.goal_pending() || self.coasting()
    }

    /// Advance animations by `dt` seconds. Frame-rate independent.
    pub fn tick(&mut self, dt: f32) {
        self.tick_ease(dt);
        self.tick_inertia(dt);
    }

    /// Ease zoom (in log space, so in/out feel even) and any offset goal.
    fn tick_ease(&mut self, dt: f32) {
        let zoom_pending = self.zoom_pending();
        if !zoom_pending && !self.goal_pending() {
            self.zoom = self.target_zoom;
            if let Some(g) = self.offset_goal.take() {
                self.offset = g;
            }
            return;
        }
        let t = 1.0 - (-ZOOM_SMOOTHING * dt).exp();
        if zoom_pending {
            let old = self.zoom;
            let lz = self.zoom.ln();
            self.zoom = (lz + (self.target_zoom.ln() - lz) * t).exp();
            if self.offset_goal.is_none() {
                // Keep the image point under the cursor fixed while zoom changes.
                let ratio = self.zoom / old;
                let cx = self.anchor.0 - self.window.0 * 0.5;
                let cy = self.anchor.1 - self.window.1 * 0.5;
                self.offset.0 = cx + (self.offset.0 - cx) * ratio;
                self.offset.1 = cy + (self.offset.1 - cy) * ratio;
            }
        }
        if let Some(g) = self.offset_goal {
            self.offset.0 += (g.0 - self.offset.0) * t;
            self.offset.1 += (g.1 - self.offset.1) * t;
        }
        self.clamp_offset();
    }

    /// Coast after a drag release with exponential friction. Uses the exact
    /// integral of the decay, so distance does not depend on frame rate.
    fn tick_inertia(&mut self, dt: f32) {
        if !self.coasting() {
            if !self.dragging {
                self.velocity = (0.0, 0.0);
            }
            return;
        }
        let decay = (-PAN_FRICTION * dt).exp();
        let k = (1.0 - decay) / PAN_FRICTION;
        self.offset.0 += self.velocity.0 * k;
        self.offset.1 += self.velocity.1 * k;
        let (hit_x, hit_y) = self.clamp_offset();
        if hit_x {
            self.velocity.0 = 0.0;
        }
        if hit_y {
            self.velocity.1 = 0.0;
        }
        self.velocity.0 *= decay;
        self.velocity.1 *= decay;
    }

    /// Keep at least a sliver of the image on screen. Returns whether the
    /// x / y limit was hit.
    fn clamp_offset(&mut self) -> (bool, bool) {
        let half_w = self.image.0 * self.zoom * 0.5;
        let half_h = self.image.1 * self.zoom * 0.5;
        let limit_x = self.window.0 * 0.5 + half_w - 64.0;
        let limit_y = self.window.1 * 0.5 + half_h - 64.0;
        let (ox, oy) = self.offset;
        self.offset.0 = ox.clamp(-limit_x, limit_x);
        self.offset.1 = oy.clamp(-limit_y, limit_y);
        (self.offset.0 != ox, self.offset.1 != oy)
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

    /// Is a window-space point (physical pixels) on the image?
    pub fn image_contains(&self, p: (f32, f32)) -> bool {
        let cx = self.window.0 * 0.5 + self.offset.0;
        let cy = self.window.1 * 0.5 + self.offset.1;
        (p.0 - cx).abs() <= self.image.0 * self.zoom * 0.5
            && (p.1 - cy).abs() <= self.image.1 * self.zoom * 0.5
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
    fn image_contains_matches_rect() {
        let mut v = Viewer::new((1000.0, 800.0));
        v.set_image((200.0, 100.0)); // zoom 1.0, centered
        assert!(v.image_contains((500.0, 400.0)));
        assert!(v.image_contains((405.0, 355.0)));
        assert!(!v.image_contains((390.0, 400.0)));
        assert!(!v.image_contains((500.0, 460.0)));
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

    #[test]
    fn zoom_keeps_point_under_cursor() {
        let mut v = Viewer::new((1920.0, 1080.0));
        v.set_image((4000.0, 3000.0));
        let cursor = (1500.0, 300.0);
        let img_at = |v: &Viewer| {
            (
                (cursor.0 - v.window.0 * 0.5 - v.offset.0) / v.zoom,
                (cursor.1 - v.window.1 * 0.5 - v.offset.1) / v.zoom,
            )
        };
        let before = img_at(&v);
        v.zoom_at(6.0, cursor);
        for _ in 0..240 {
            v.tick(1.0 / 60.0);
        }
        let after = img_at(&v);
        assert!((before.0 - after.0).abs() < 0.5 && (before.1 - after.1).abs() < 0.5);
    }

    #[test]
    fn zoom_is_frame_rate_independent() {
        let run = |dt: f32, frames: usize| {
            let mut v = Viewer::new((1920.0, 1080.0));
            v.set_image((800.0, 600.0));
            v.zoom_at(3.0, (960.0, 540.0));
            for _ in 0..frames {
                v.tick(dt);
            }
            v.zoom
        };
        assert!((run(1.0 / 60.0, 12) - run(1.0 / 120.0, 24)).abs() < 1e-3);
    }

    #[test]
    fn fling_coasts_then_stops() {
        let mut v = Viewer::new((1920.0, 1080.0));
        v.set_image((4000.0, 3000.0));
        v.begin_drag();
        for _ in 0..10 {
            v.drag_by(10.0, 0.0, 1.0 / 120.0); // ~1200 px/s
        }
        let released_at = v.offset.0;
        v.end_drag(0.005);
        assert!(v.is_animating());
        for _ in 0..600 {
            v.tick(1.0 / 60.0);
        }
        assert!(!v.is_animating());
        assert!(v.offset.0 > released_at + 50.0);
    }

    #[test]
    fn pause_before_release_cancels_fling() {
        let mut v = Viewer::new((1920.0, 1080.0));
        v.set_image((4000.0, 3000.0));
        v.begin_drag();
        v.drag_by(30.0, 0.0, 1.0 / 120.0);
        v.end_drag(0.3);
        assert!(!v.is_animating());
    }

    #[test]
    fn animated_fit_returns_to_center() {
        let mut v = Viewer::new((1920.0, 1080.0));
        v.set_image((4000.0, 3000.0));
        v.zoom_at(6.0, (1500.0, 300.0));
        for _ in 0..120 {
            v.tick(1.0 / 60.0);
        }
        v.fit_animated();
        for _ in 0..300 {
            v.tick(1.0 / 60.0);
        }
        assert!(!v.is_animating());
        assert_eq!(v.offset, (0.0, 0.0));
        assert!((v.zoom - v.fit_zoom()).abs() < 1e-3);
    }
}
