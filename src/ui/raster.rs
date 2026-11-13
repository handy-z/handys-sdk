//! A small CPU rasterizer for turning 3D models in a dashboard picture slot: a few thousand triangles into a slot of
//! about 100x60 pixels every tick costs well under a millisecond, which keeps Direct3D out of the dashboard.

/// One triangle: corners in model space (Y up), per-corner normals in model space (an all-zero or non-finite one uses
/// the face normal, so `[[0.0; 3]; 3]` draws it flat), color 0xRRGGBB, `glow` drawn mostly unlit, brightened and with a
/// soft halo (like Roblox Neon).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tri {
    pub p: [[f32; 3]; 3],
    pub n: [[f32; 3]; 3],
    pub rgb: u32,
    pub glow: bool,
}

const SS: usize = 3; // samples per pixel along each axis, box-filtered down for smooth edges
const PITCH: f32 = 0.4; // radians the camera looks down from
const AMBIENT: f32 = 0.3;
const GLOW_AMBIENT: f32 = 0.7; // Neon: mostly its own light, a little shape kept so it doesn't read as a flat silhouette
const MARGIN: f32 = 0.95; // of the half-size the bounding sphere fills
const HALO_RADIUS: f32 = 0.05; // of the buffer's smaller side
const HALO: f32 = 1.2; // the blur is about half the color at a silhouette, so the halo starts at ~0.6 of it
const GLOW_BIT: u32 = 1 << 24; // marks a sample's packed color as a glow surface

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn unit(v: [f32; 3]) -> Option<[f32; 3]> {
    let len = dot(v, v).sqrt();
    (len.is_finite() && len > 1e-12).then(|| v.map(|c| c / len))
}

/// One box pass of radius `r` along rows, then along columns; pixels past the edges count as black.
fn box_blur(img: &mut [[f32; 3]], w: usize, h: usize, r: usize) {
    let mut line = vec![[0.0f32; 3]; w.max(h)];
    let norm = 1.0 / (2 * r + 1) as f32;
    for (len, lines, step, next) in [(w, h, 1, w), (h, w, w, 1)] {
        for l in 0..lines {
            for (i, v) in line[..len].iter_mut().enumerate() {
                *v = img[l * next + i * step];
            }
            let mut sum = [0.0f32; 3];
            for v in &line[..r.min(len)] {
                (0..3).for_each(|k| sum[k] += v[k]);
            }
            for i in 0..len {
                if i + r < len {
                    (0..3).for_each(|k| sum[k] += line[i + r][k]);
                }
                img[l * next + i * step] = sum.map(|v| v * norm);
                if i >= r {
                    (0..3).for_each(|k| sum[k] -= line[i - r][k]);
                }
            }
        }
    }
}

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
    // model to view space: x right, y up, z toward the camera
    let turn = |v: [f32; 3]| {
        let (x, z) = (v[0] * yc + v[2] * ys, -v[0] * ys + v[2] * yc);
        [x, v[1] * pc - z * ps, v[1] * ps + z * pc]
    };
    // then x, y to samples (y down); z scaled alike so the face normal below comes out undistorted
    let view = |v: [f32; 3]| {
        let [x, y, z] = turn([v[0] - c[0], v[1] - c[1], v[2] - c[2]]);
        [sw as f32 / 2.0 + x * scale, sh as f32 / 2.0 - y * scale, z * scale]
    };
    // upper left and partly toward the camera: a light along the view lit every face alike, one from the side left
    // camera-facing faces dim
    let light = unit([-0.6, 0.6, 0.8]).unwrap_or_default();
    let mut depth = vec![f32::NEG_INFINITY; sw * sh];
    let mut color = vec![0u32; sw * sh];
    let mut any_glow = false;
    for t in tris.iter().filter(finite) {
        let [a, b, d] = t.p.map(view);
        let area = (b[0] - a[0]) * (d[1] - a[1]) - (b[1] - a[1]) * (d[0] - a[0]);
        if area.abs() < 1e-6 {
            continue; // degenerate, or edge-on
        }
        any_glow |= t.glow;
        let corners = t.n.map(|n| unit(n).map(turn));
        let face = {
            // in view space with y up (undo the sample flip)
            let (u, v) = (
                [b[0] - a[0], a[1] - b[1], b[2] - a[2]],
                [d[0] - a[0], a[1] - d[1], d[2] - a[2]],
            );
            let n = unit([
                u[1] * v[2] - u[2] * v[1],
                u[2] * v[0] - u[0] * v[2],
                u[0] * v[1] - u[1] * v[0],
            ])
            .unwrap_or_default();
            // the winding is arbitrary: face the way the given corner normals do, so blending with them can't cancel
            let given = corners
                .iter()
                .flatten()
                .fold([0.0; 3], |s, c| [s[0] + c[0], s[1] + c[1], s[2] + c[2]]);
            if dot(n, given) < 0.0 { n.map(|v| -v) } else { n }
        };
        let [n0, n1, n2] = corners.map(|c| c.unwrap_or(face));
        let ambient = if t.glow { GLOW_AMBIENT } else { AMBIENT };
        let base = [16, 8, 0].map(|s| ((t.rgb >> s) & 0xff) as f32);
        let flag = if t.glow { GLOW_BIT } else { 0 };
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
                if z <= depth[i] {
                    continue;
                }
                depth[i] = z;
                let n = [0, 1, 2].map(|k| w0 * n0[k] + w1 * n1[k] + w2 * n2[k]);
                // |n.l| so either winding is lit
                let lit = (dot(n, light) / dot(n, n).sqrt()).abs();
                let shade = ambient + (1.0 - ambient) * if lit.is_finite() { lit.min(1.0) } else { 0.0 };
                let rgb = base.map(|ch| {
                    let v = ch * shade;
                    (if t.glow { v + (255.0 - v) * 0.3 } else { v })
                        .round()
                        .clamp(0.0, 255.0) as u32
                });
                color[i] = flag | (rgb[0] << 16) | (rgb[1] << 8) | rgb[2];
            }
        }
    }
    let n = (SS * SS) as u32;
    let mut glow = vec![[0.0f32; 3]; if any_glow { w * h } else { 0 }];
    for y in 0..h {
        for x in 0..w {
            let (mut sum, mut lit, mut hits) = ([0u32; 3], [0u32; 3], 0u32);
            for sy in 0..SS {
                for sx in 0..SS {
                    let i = (y * SS + sy) * sw + x * SS + sx;
                    if depth[i] > f32::NEG_INFINITY {
                        hits += 1;
                        let ch = [color[i] & 0xff, (color[i] >> 8) & 0xff, (color[i] >> 16) & 0xff];
                        (0..3).for_each(|k| sum[k] += ch[k]);
                        if color[i] & GLOW_BIT != 0 {
                            (0..3).for_each(|k| lit[k] += ch[k]);
                        }
                    }
                }
            }
            // uncovered samples count as transparent black: the averages are already premultiplied
            let o = (y * w + x) * 4;
            out[o..o + 4].copy_from_slice(&[sum[0] / n, sum[1] / n, sum[2] / n, hits * 255 / n].map(|v| v as u8));
            if any_glow {
                glow[y * w + x] = lit.map(|v| v as f32 / n as f32);
            }
        }
    }
    if any_glow {
        // two box passes of half the radius make a tent: a small glowing spot gets a round-ish halo, not a square
        let r = ((w.min(h) as f32 * HALO_RADIUS / 2.0).round() as usize).max(1);
        box_blur(&mut glow, w, h, r);
        box_blur(&mut glow, w, h, r);
        for (px, g) in out.as_chunks_mut::<4>().0.iter_mut().zip(&glow) {
            // premultiplied "over" with the halo behind the model: it lights the background and soft edges, never
            // washes out the faces, and color <= alpha holds because it holds for both layers
            let hc = g.map(|v| (v * HALO).min(255.0));
            let ha = hc[0].max(hc[1]).max(hc[2]);
            let behind = f32::from(255 - px[3]) / 255.0;
            for (k, v) in hc.into_iter().chain([ha]).enumerate() {
                px[k] += (v * behind) as u8;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tri(p: [[f32; 3]; 3], rgb: u32, glow: bool) -> Tri {
        Tri {
            p,
            n: [[0.0; 3]; 3],
            rgb,
            glow,
        }
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

    fn premultiplied(b: &[u8]) -> bool {
        b.chunks(4).all(|p| p[0] <= p[3] && p[1] <= p[3] && p[2] <= p[3])
    }

    // a unit UV sphere, coarse enough that flat facets show; `smooth` gives each corner its outward normal
    fn sphere(smooth: bool) -> Vec<Tri> {
        let (stacks, slices) = (8, 12);
        let at = |i: usize, j: usize| {
            let (t, f) = (
                std::f32::consts::PI * i as f32 / stacks as f32,
                std::f32::consts::TAU * j as f32 / slices as f32,
            );
            [t.sin() * f.cos(), t.cos(), t.sin() * f.sin()]
        };
        let mut out = Vec::new();
        for i in 0..stacks {
            for j in 0..slices {
                for p in [
                    [at(i, j), at(i + 1, j), at(i + 1, j + 1)],
                    [at(i, j), at(i + 1, j + 1), at(i, j + 1)],
                ] {
                    let mut t = tri(p, 0xc0c0c0, false);
                    if smooth {
                        t.n = p;
                    }
                    out.push(t);
                }
            }
        }
        out
    }

    #[test]
    fn smooth_normals_blend_across_edges() {
        // the biggest brightness step between two opaque neighbors
        let step = |model: &[Tri]| {
            let b = render(model, 80, 80, 0.3);
            assert!(premultiplied(&b));
            let mut worst = 0;
            for y in 0..80 {
                for x in 0..79 {
                    let (p, q) = (px(&b, 80, x, y), px(&b, 80, x + 1, y));
                    if p[3] == 255 && q[3] == 255 {
                        worst = worst.max((i32::from(p[1]) - i32::from(q[1])).abs());
                    }
                }
            }
            worst
        };
        let (smooth, flat) = (step(&sphere(true)), step(&sphere(false)));
        assert!(smooth * 2 < flat, "{smooth} {flat}");
    }

    #[test]
    fn missing_normals_fall_back_to_the_face() {
        let flat = render(&[big(false)], 30, 30, 0.0);
        let close = |b: Vec<u8>| flat.iter().zip(&b).all(|(x, y)| x.abs_diff(*y) <= 1);
        for n in [
            [[0.0, 0.0, 1.0]; 3],
            [[0.0, 0.0, -2.0]; 3],                             // either side, any length
            [[0.0, 0.0, 1.0], [0.0; 3], [f32::NAN, 0.0, 0.0]], // given where known, the face elsewhere
        ] {
            assert!(close(render(&[Tri { n, ..big(false) }], 30, 30, 0.0)), "{n:?}");
        }
    }

    #[test]
    fn glow_has_a_halo() {
        let (lit, glow) = (render(&[big(false)], 60, 60, 0.0), render(&[big(true)], 60, 60, 0.0));
        assert!(premultiplied(&lit) && premultiplied(&glow));
        // the halo reaches past the silhouette, where the plain triangle leaves the background clear
        let halo = lit
            .chunks(4)
            .zip(glow.chunks(4))
            .filter(|(l, g)| l[3] == 0 && g[3] > 0)
            .count();
        assert!(halo > 20, "{halo}");
        assert_eq!(px(&glow, 60, 0, 0), [0; 4]); // and fades out
    }
}
