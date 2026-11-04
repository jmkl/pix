# Pix

A minimal Picasa-style image viewer: a fullscreen, dimmed, transparent window
with the image in the center. Rust, `winit` + `wgpu`. See `PLAN.md` for the design.

## Run

```
cargo run --release -- path\to\image.jpg
```

Pix lists the image's folder (natural sort) so the arrow keys move between its
neighbors. In debug builds a console stays open for logs; set `RUST_LOG=info`
to see what the renderer picked.

## Controls

| Input              | Action                                        |
| ------------------ | --------------------------------------------- |
| Mouse wheel        | Zoom, anchored on the cursor                  |
| Left-click drag    | Pan                                           |
| Left / Right arrow | Previous / next image in the folder           |
| `0` / `1`          | Fit to screen / 100% zoom                     |
| Right-click        | Copy the full-resolution image to clipboard   |
| Esc                | Quit                                          |

## Build a release

```
cargo build --release
```

The result is `target\release\pix.exe` (a single file). On Windows it runs
without a console window and carries the app icon, which is generated from
`src/icon.rs` by `build.rs`. Embedding the icon needs the Windows SDK resource
compiler (installed with the Visual Studio C++ build tools); if it is missing the
build still succeeds, just without the .exe icon.

## Install with Nushell (recommended)

[Nushell](https://www.nushell.sh) is the only extra requirement. From the project folder:

```
nu packaging/install.nu                # build + install for the current user
nu packaging/install.nu --add-to-path  # Windows: also put it on your PATH
nu packaging/install.nu --no-build     # install an existing release build
nu packaging/install.nu --uninstall    # remove it again
```

- **Windows:** installs to `%LOCALAPPDATA%\Programs\Pix` and registers the image
  file associations (see below for the final Settings step).
- **Linux:** installs to `~/.local/bin` and adds a desktop entry.
- **macOS:** installs to `~/.local/bin`.

Use `--prefix <dir>` to install somewhere else, and `--no-register` to skip the
Windows file associations. Nothing needs admin rights.

## Open images by double-clicking (Windows, manual)

The install script above does this for you; use these steps only if you copy
`pix.exe` around yourself.

1. Put `pix.exe` where you want to keep it, next to `packaging\windows\register.ps1`
   (or pass `-Exe`).
2. Run:
   ```
   powershell -ExecutionPolicy Bypass -File register.ps1 -Exe C:\Tools\pix.exe
   ```
3. Open Settings > Apps > Default apps > Pix and choose it for the image types
   you want. Windows does not allow a script to set the default for you.

To undo: `powershell -ExecutionPolicy Bypass -File unregister.ps1`. Both scripts only
touch the current user's registry (no admin needed).

## Linux

Copy `packaging/linux/pix.desktop` to `~/.local/share/applications/` and make
sure `pix` is on your `PATH`. A compositing window manager is needed for the
transparent backdrop.

## Supported formats

JPEG, PNG, GIF (first frame), WebP, BMP, TIFF.
