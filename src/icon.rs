//! Procedural app icon, shared by the window (runtime) and `build.rs` (the
//! .exe icon), so there is a single source of truth and no binary asset.
//! Only depends on `std`.

/// RGBA8, top-down, `size` x `size` pixels. Anti-aliased by supersampling.
pub fn rgba(size: u32) -> Vec<u8> {
    const SS: u32 = 4; // SS x SS samples per pixel
    let mut out = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let (mut r, mut g, mut b, mut a) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
            for sy in 0..SS {
                for sx in 0..SS {
                    let u = (x as f32 + (sx as f32 + 0.5) / SS as f32) / size as f32;
                    let v = (y as f32 + (sy as f32 + 0.5) / SS as f32) / size as f32;
                    if let Some([cr, cg, cb]) = sample(u, v) {
                        r += cr;
                        g += cg;
                        b += cb;
                        a += 1.0;
                    }
                }
            }
            let n = (SS * SS) as f32;
            if a > 0.0 {
                out.push((r / a * 255.0 + 0.5) as u8);
                out.push((g / a * 255.0 + 0.5) as u8);
                out.push((b / a * 255.0 + 0.5) as u8);
                out.push((a / n * 255.0 + 0.5) as u8);
            } else {
                out.extend_from_slice(&[0, 0, 0, 0]);
            }
        }
    }
    out
}

fn hex(c: u32) -> [f32; 3] {
    [
        ((c >> 16) & 0xff) as f32 / 255.0,
        ((c >> 8) & 0xff) as f32 / 255.0,
        (c & 0xff) as f32 / 255.0,
    ]
}

fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

/// Color at a point in unit space (0..1, y down), or `None` if transparent.
fn sample(u: f32, v: f32) -> Option<[f32; 3]> {
    // Rounded-square tile.
    let half = 0.47;
    let radius = 0.2;
    let qx = (u - 0.5).abs() - (half - radius);
    let qy = (v - 0.5).abs() - (half - radius);
    let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt() + qx.max(qy).min(0.0) - radius;
    if outside > 0.0 {
        return None;
    }

    let mut color = mix(hex(0x2b2d3a), hex(0x14151c), v);

    // Sun.
    let (dx, dy) = (u - 0.68, v - 0.34);
    if (dx * dx + dy * dy).sqrt() <= 0.09 {
        color = hex(0xffb454);
    }

    // Two mountains: (peak x, peak y, half width, color).
    let base = 0.82;
    for (px, py, hw, c) in [(0.38, 0.38, 0.30, 0x3d8f9a), (0.66, 0.52, 0.26, 0x5fc9c0)] {
        if v >= py && v <= base && (u - px).abs() <= (v - py) * (hw / (base - py)) {
            color = hex(c);
        }
    }

    Some(color)
}
