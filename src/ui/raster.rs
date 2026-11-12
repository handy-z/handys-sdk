//! A small CPU rasterizer for turning 3D models in a dashboard picture slot: a few thousand triangles into a slot of
//! about 100x60 pixels every tick costs well under a millisecond, which keeps Direct3D out of the dashboard.

/// One triangle: corners in model space (Y up), color 0xRRGGBB, `glow` drawn mostly unlit and brightened (like Roblox
/// Neon).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tri {
    pub p: [[f32; 3]; 3],
    pub rgb: u32,
    pub glow: bool,
}

const SS: usize = 3; // samples per pixel along each axis, box-filtered down for smooth edges
const PITCH: f32 = 0.4; // radians the camera looks down from
const AMBIENT: f32 = 0.3;
const GLOW_AMBIENT: f32 = 0.7; // Neon: mostly its own light, a little shape kept so it doesn't read as a flat silhouette
const MARGIN: f32 = 0.95; // of the half-size the bounding sphere fills

/// The triangles drawn into a w x h premultiplied BGRA buffer (row-major, 4*w bytes per row), transparent background:
/// turned `yaw` radians about the vertical (Y, up) axis around the model's bounding-box center, seen from a fixed
/// slight elevation, fitted to the buffer by its bounding sphere (so its size stays fixed while it turns).
pub fn render(tris: &[Tri], w: u32, h: u32, yaw: f32) -> Vec<u8> {
    let (w, h) = (w as usize, h as usize);
    let mut out = vec![0u8; w * h * 4];
    let finite = |t: &&Tri| t.p.iter().flatten().all(|v| v.is_finite());
    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
    for v in tris.iter().filter(finite).flat_map(|t| t.p) {
        for k in 0..3 {
            lo[k] = lo[k].min(v[k]);
            hi[k] = hi[k].max(v[k]);
        }
    }
    let c = [0, 1, 2].map(|k| (lo[k] + hi[k]) / 2.0);
    let radius = tris
        .iter()
        .filter(finite)
        .flat_map(|t| t.p)
        .map(|v| ((v[0] - c[0]).powi(2) + (v[1] - c[1]).powi(2) + (v[2] - c[2]).powi(2)).sqrt())
        .fold(0.0f32, f32::max);
    if w == 0 || h == 0 || radius <= 0.0 || !radius.is_finite() {
        return out;
    }
    let (sw, sh) = (w * SS, h * SS);
    let scale = sw.min(sh) as f32 / 2.0 * MARGIN / radius;
    let (ys, yc) = yaw.sin_cos();
    let (ps, pc) = PITCH.sin_cos();
    // view space: x right, y up, z toward the camera; then x, y to samples (y down) and z kept for depth
    let view = |v: [f32; 3]| {
        let (x, y, z) = (v[0] - c[0], v[1] - c[1], v[2] - c[2]);
        let (x, z) = (x * yc + z * ys, -x * ys + z * yc);
        let (y, z) = (y * pc - z * ps, y * ps + z * pc);
        [sw as f32 / 2.0 + x * scale, sh as f32 / 2.0 - y * scale, z]
    };
    let light = {
        let l = [-0.6f32, 0.6, 0.5]; // upper left and only half toward the camera: a light along the view lit every face alike
        let n = (l[0] * l[0] + l[1] * l[1] + l[2] * l[2]).sqrt();
        l.map(|v| v / n)
    };
    let mut depth = vec![f32::NEG_INFINITY; sw * sh];
    let mut color = vec![0u32; sw * sh];
    for t in tris.iter().filter(finite) {
        let [a, b, d] = t.p.map(view);
        let area = (b[0] - a[0]) * (d[1] - a[1]) - (b[1] - a[1]) * (d[0] - a[0]);
        if area.abs() < 1e-6 {
            continue; // degenerate, or edge-on
        }
        let shade = {
            // normal in view space with y up (undo the sample flip), |n.l| so either winding is lit
            let (u, v) = (
                [b[0] - a[0], a[1] - b[1], b[2] - a[2]],
                [d[0] - a[0], a[1] - d[1], d[2] - a[2]],
            );
            let n = [
                u[1] * v[2] - u[2] * v[1],
                u[2] * v[0] - u[0] * v[2],
                u[0] * v[1] - u[1] * v[0],
            ];
            let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
            let dot = (n[0] * light[0] + n[1] * light[1] + n[2] * light[2]) / len;
            let lit = if dot.is_finite() { dot.abs() } else { 0.0 };
            let ambient = if t.glow { GLOW_AMBIENT } else { AMBIENT };
            ambient + (1.0 - ambient) * lit
        };
        let rgb = [16, 8, 0].map(|s| ((t.rgb >> s) & 0xff) as f32).map(|ch| {
            let v = ch * shade;
            if t.glow { v + (255.0 - v) * 0.3 } else { v }
        });
        let px = rgb.map(|v| v.round().clamp(0.0, 255.0) as u32);
        let packed = (px[0] << 16) | (px[1] << 8) | px[2];
        let x0 = a[0].min(b[0]).min(d[0]).floor().max(0.0) as usize;
        let x1 = (a[0].max(b[0]).max(d[0]).ceil().max(0.0) as usize).min(sw);
        let y0 = a[1].min(b[1]).min(d[1]).floor().max(0.0) as usize;
        let y1 = (a[1].max(b[1]).max(d[1]).ceil().max(0.0) as usize).min(sh);
        for y in y0..y1 {
            let py = y as f32 + 0.5;
            for x in x0..x1 {
                let px = x as f32 + 0.5;
                // barycentrics divided by the signed area: all >= 0 inside whichever way it's wound
                let w0 = ((b[0] - px) * (d[1] - py) - (b[1] - py) * (d[0] - px)) / area;
                let w1 = ((d[0] - px) * (a[1] - py) - (d[1] - py) * (a[0] - px)) / area;
                let w2 = 1.0 - w0 - w1;
                if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                    continue;
                }
                let z = w0 * a[2] + w1 * b[2] + w2 * d[2];
                let i = y * sw + x;
                if z > depth[i] {
                    depth[i] = z;
                    color[i] = packed;
                }
            }
        }
    }
    let n = (SS * SS) as u32;
    for y in 0..h {
        for x in 0..w {
            let (mut sum, mut hits) = ([0u32; 3], 0u32);
            for sy in 0..SS {
                for sx in 0..SS {
                    let i = (y * SS + sy) * sw + x * SS + sx;
                    if depth[i] > f32::NEG_INFINITY {
                        hits += 1;
                        sum[0] += color[i] & 0xff;
                        sum[1] += (color[i] >> 8) & 0xff;
                        sum[2] += color[i] >> 16;
                    }
                }
            }
            // uncovered samples count as transparent black: the averages are already premultiplied
            let o = (y * w + x) * 4;
            out[o..o + 4].copy_from_slice(&[sum[0] / n, sum[1] / n, sum[2] / n, hits * 255 / n].map(|v| v as u8));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tri(p: [[f32; 3]; 3], rgb: u32, glow: bool) -> Tri {
        Tri { p, rgb, glow }
    }

    fn big(glow: bool) -> Tri {
        tri([[-1.0, -1.0, 0.0], [1.0, -1.0, 0.0], [0.0, 1.0, 0.0]], 0xff8000, glow)
    }

    fn px(buf: &[u8], w: u32, x: u32, y: u32) -> [u8; 4] {
        let o = ((y * w + x) * 4) as usize;
        buf[o..o + 4].try_into().unwrap()
    }

    // a 2 x 1 x 1 box, half its faces wound the other way
    fn boxed() -> Vec<Tri> {
        let v = |i: usize| {
            [
                if i & 1 != 0 { 1.0 } else { -1.0 },
                (i >> 1 & 1) as f32 * 1.0,
                (i >> 2 & 1) as f32,
            ]
        };
        let quads = [
            [0, 1, 3, 2],
            [4, 5, 7, 6],
            [0, 1, 5, 4],
            [2, 3, 7, 6],
            [0, 2, 6, 4],
            [1, 3, 7, 5],
        ];
        quads
            .iter()
            .flat_map(|q| {
                [
                    tri([v(q[0]), v(q[1]), v(q[2])], 0x40a0ff, false),
                    tri([v(q[2]), v(q[3]), v(q[0])], 0x40a0ff, false),
                ]
            })
            .collect()
    }

    #[test]
    fn big_triangle_fills_the_center_not_the_corner() {
        let b = render(&[big(false)], 40, 40, 0.0);
        assert_eq!(b.len(), 40 * 40 * 4);
        let [bl, g, r, a] = px(&b, 40, 20, 20);
        assert_eq!(a, 255);
        assert!(r > 150 && g > 60 && g < r && bl < 10, "{r} {g} {bl}"); // shaded orange
        assert_eq!(px(&b, 40, 0, 0), [0; 4]);
        // edges are blended, and stay premultiplied
        assert!(b.chunks(4).any(|p| p[3] > 0 && p[3] < 255));
        assert!(b.chunks(4).all(|p| p[0] <= p[3] && p[1] <= p[3] && p[2] <= p[3]));
    }

    #[test]
    fn nothing_to_draw_is_transparent() {
        assert_eq!(render(&[], 12, 7, 0.0), vec![0; 12 * 7 * 4]);
        assert!(render(&[big(false)], 0, 7, 0.0).is_empty());
        let junk = [
            tri([[0.0; 3]; 3], 0xffffff, false),
            tri(
                [[f32::NAN, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
                0xffffff,
                false,
            ),
        ];
        assert_eq!(render(&junk, 12, 7, 0.0), vec![0; 12 * 7 * 4]);
        // a NaN triangle beside a real one is skipped, the real one still drawn
        assert!(
            render(&[junk[1], big(false)], 12, 7, 0.0)
                .chunks(4)
                .any(|p| p[3] == 255)
        );
    }

    #[test]
    fn size_stays_while_turning() {
        let model = boxed();
        let opaque = |yaw| {
            let b = render(&model, 120, 60, yaw);
            // the bounding sphere keeps it off the edges at any angle
            for x in 0..120 {
                assert_eq!(px(&b, 120, x, 0)[3], 0);
                assert_eq!(px(&b, 120, x, 59)[3], 0);
            }
            b.chunks(4).filter(|p| p[3] > 128).count() as f32
        };
        let (a, b) = (opaque(0.0), opaque(1.0));
        assert!(a > 200.0 && (0.7..1.4).contains(&(b / a)), "{a} {b}");
    }

    #[test]
    fn glow_is_brighter_than_lit() {
        let sum = |t| {
            px(&render(&[t], 20, 20, 0.0), 20, 10, 10)[..3]
                .iter()
                .map(|&v| u32::from(v))
                .sum::<u32>()
        };
        assert!(sum(big(true)) > sum(big(false)));
    }
}
