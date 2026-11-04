# Pix — a minimal Picasa-style image viewer in Rust

A fullscreen, dimmed, transparent image viewer. Open an image, it floats in the
center over a darkened desktop. Smooth zoom and pan, arrow-key navigation,
right-click to copy, Esc to quit.

## Goals

- Smoothness first: 60+ fps zoom/pan, instant-feeling navigation.
- Near-zero CPU/GPU use while idle.
- Small, dependency-light, single binary.

## Non-goals (for now)

- Editing, organizing, tagging, face recognition.
- Thumbnails grid / library view.
- Animated formats (GIF/APNG playback).

## Controls

| Input              | Action                                        |
| ------------------ | --------------------------------------------- |
| Mouse wheel        | Zoom, anchored on the cursor                  |
| Left-click drag    | Pan                                           |
| Left / Right arrow | Previous / next image in the same folder      |
| Right-click        | Copy the full-resolution image to clipboard   |
| Esc                | Quit                                          |
| `0` / `1`          | Fit to screen / 100% zoom                     |

## Stack

| Concern    | Choice                         | Why                                              |
| ---------- | ------------------------------ | ------------------------------------------------ |
| Window     | `winit`                        | Borderless, fullscreen, `with_transparent(true)` |
| GPU        | `wgpu`                         | One textured quad; zoom/pan are uniforms         |
| Decode     | `image` (+ `zune-jpeg` later)  | Wide format support, fast JPEG path available    |
| Clipboard  | `arboard`                      | Cross-platform RGBA image copy                   |
| Sorting    | `natord`                       | Natural order: `img2` before `img10`             |
| Threads    | `std::thread` + `mpsc`         | Simple background decode, no async runtime       |

## Architecture

```
src/
  main.rs       entry point, CLI arg, event loop
  app.rs        winit ApplicationHandler: input -> state, redraw scheduling
  viewer.rs     pure view state: zoom/pan math, smoothing (no GPU, unit-tested)
  navigator.rs  folder listing, filtering, natural sort, next/prev
  loader.rs     background decode thread, LRU cache, preloading
  renderer.rs   wgpu surface, pipeline, texture upload, draw
  clipboard.rs  copy RGBA pixels on a worker thread
  shader.wgsl   textured quad with scale/offset uniform
```

Data flow:

```
winit event -> app.rs -> viewer.rs (update target zoom/offset)
                      -> navigator.rs (change index) -> loader.rs (request decode)
loader thread -> decoded RGBA -> app.rs -> renderer.rs (upload texture)
RedrawRequested -> viewer.tick(dt) -> renderer.draw(view uniform)
```

## Key design decisions

1. **Event-driven rendering.** `ControlFlow::Wait`. Request a redraw only on
   input or while `viewer.is_animating()`. Idle cost ≈ 0.
2. **Zoom smoothing.** Keep `target_zoom` and `zoom`. Each frame ease
   `zoom` toward `target_zoom` with a frame-rate independent factor
   (`1 - exp(-k * dt)`). Anchor on the cursor so the point under it stays fixed.
3. **Pan applies directly** (no easing) so dragging feels attached to the
   mouse. Clamp so some part of the image always stays visible.
4. **Transparent dim.** Premultiplied-alpha surface; clear color
   `rgba(0,0,0,0.75)` premultiplied. Image quad drawn over it.
5. **Preload neighbors.** Decode index±1 ahead in the background, keep a small
   LRU (3–5 images). Arrow keys then only swap a texture.
6. **Huge images.** If larger than `max_texture_dimension_2d`, downscale for the
   GPU copy; keep the original pixels around for clipboard copy.
7. **Sampling.** Linear + mipmaps when zoomed out; nearest when zoom > 3x so
   pixels are inspectable.
8. **Clipboard on a worker thread** so a large copy never stalls a frame.
9. **EXIF orientation** applied at decode time.

## Milestones

`[~]` = code written, not yet compiled or run on real hardware.

- [~] **M1 — Window.** Transparent fullscreen window, dim backdrop, Esc quits.
- [~] **M2 — Display.** Load the CLI-provided image, upload, fit-to-screen, centered.
- [~] **M3 — Zoom/pan.** Cursor-anchored smooth zoom, left-click-drag pan, clamping.
- [~] **M4 — Navigation.** Folder scan, natural sort, arrows with wraparound,
      background decode, preload, LRU cache.
- [~] **M5 — Clipboard.** Right-click copy of full-res pixels.
- [~] **M6 — Polish.** EXIF rotation, mipmaps, nearest-neighbor at high zoom,
      HiDPI correctness (all math is in physical pixels), skip undecodable
      files. Deferred: an on-screen text toast (needs a text renderer).
- [~] **M7 — Packaging.** Release profile, no console window on Windows, generated
      app icon (exe + window), Windows file-association scripts, Linux .desktop, README.

## Performance checklist

- [ ] Release profile: `lto = "fat"`, `codegen-units = 1`, `panic = "abort"`.
- [ ] `PresentMode::Fifo` (vsync), surface alpha `PreMultiplied`.
- [ ] No allocation in the per-frame path.
- [ ] Decode never runs on the UI thread.
- [ ] Texture upload is the only heavy per-navigation GPU work.
- [ ] Measure: time-to-first-pixel, navigation latency, idle CPU %.

## Platform notes

- **Windows 10/11, macOS:** transparency works out of the box.
- **Linux:** needs a compositing window manager (Wayland compositors and most
  X11 desktops qualify).
- Clipboard images are copied as pixels, not file paths.

## Risks

- Transparent windows and `wgpu` alpha modes vary by backend/driver; test
  `PreMultiplied` vs `PostMultiplied` and fall back to an opaque dim if needed.
- `wgpu` and `winit` APIs move quickly; pin versions in `Cargo.toml`.
- Very large or exotic formats (RAW, HEIC) are out of scope for v1.
