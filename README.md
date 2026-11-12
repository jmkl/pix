# Pix Image Viewer

- **Left-drag** – move the window (no title bar); once the image is larger than the window, or in fullscreen, pans the image instead
- **Scroll wheel** – smooth zoom. The image eases to the new size; the window follows (grows first, shrinks after the image settles) until it reaches the monitor size, then the image zooms inside it, anchored at the cursor
- **Right-click** – toggle fullscreen

Requires Rust 1.95+ (egui/eframe 0.37).

Build: `cargo build --release`   (fat LTO, 1 codegen unit: slow build, fastest binary)
Run:   `target/release/pix path/to/image.jpg`

Formats: JPEG, PNG, BMP (zune-image fast path); GIF (first frame), WebP, ICO, TIFF (image crate).
EXIF rotation is applied.
