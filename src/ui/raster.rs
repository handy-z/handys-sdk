//! A small CPU rasterizer for drawing 3D models into a dashboard picture slot: a few thousand triangles into a slot of
//! about 200x100 pixels costs a few milliseconds, which keeps Direct3D out of the dashboard.

/// One triangle: corners in camera space (x right, y up, the camera at the origin looking down -z), per-corner normals
/// in the same space (an all-zero or non-finite one uses the face normal, so `[[0.0; 3]; 3]` draws it flat), color
/// 0xRRGGBB, `glow` drawn mostly unlit, brightened and with a soft halo (like Roblox Neon). `tex` is 1 + an index into
/// render's textures (0 for none), sampled at the corners' `uv` (0..1, v down) and laid over the color by its alpha.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Tri {
    pub p: [[f32; 3]; 3],
    pub n: [[f32; 3]; 3],
    pub rgb: u32,
    pub glow: bool,
    pub uv: [[f32; 2]; 3],
    pub tex: u32,
    /// Corner colors (r, g, b, a) the color is multiplied by, as Roblox tints a mesh's vertex colors by its part's;
    /// a corner with alpha 0 counts as white (none given).
    pub vc: [[u8; 4]; 3],
    /// How see-through, 0 solid to 1 invisible (Roblox Transparency): drawn after the solid ones, over what is behind.
    pub clear: f32,
    /// Glossiness 0..1: a white glint where the light reflects toward the camera.
    pub shine: f32,
    /// ForceField-like: its see-through surface ripples with Scene::time.
    pub shimmer: bool,
}

/// A straight-alpha RGBA image, row-major.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Texture {
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
}

impl Texture {
    /// Bilinear, wrapping; [r, g, b, a] 0..1. An empty texture (one still loading) is clear, so the color shows.
    pub fn sample(&self, u: f32, v: f32) -> [f32; 4] {
        let (w, h) = (self.w as usize, self.h as usize);
        if w == 0 || h == 0 || self.rgba.len() < w * h * 4 || !u.is_finite() || !v.is_finite() {
            return [0.0; 4];
        }
        let (x, y) = (u.rem_euclid(1.0) * w as f32 - 0.5, v.rem_euclid(1.0) * h as f32 - 0.5);
        let (fx, fy) = (x - x.floor(), y - y.floor());
        let (x0, y0) = (x.floor() as isize, y.floor() as isize);
        let at = |dx: isize, dy: isize| {
            let (xi, yi) = (
                (x0 + dx).rem_euclid(w as isize) as usize,
                (y0 + dy).rem_euclid(h as isize) as usize,
            );
            let o = (yi * w + xi) * 4;
            [0, 1, 2, 3].map(|k| f32::from(self.rgba[o + k]) / 255.0)
        };
        let (a, b, c, d) = (at(0, 0), at(1, 0), at(0, 1), at(1, 1));
        [0, 1, 2, 3].map(|k| {
            let top = a[k] + (b[k] - a[k]) * fx;
            let bottom = c[k] + (d[k] - c[k]) * fx;
            top + (bottom - top) * fy
        })
    }
}

/// How the triangles are seen: a perspective camera with this vertical field of view (radians; 0 for orthographic),
/// and Roblox-style light: `ambient` plus `color` times the diffuse term of a light shining along `dir` (camera
/// space), each channel 0..1 and the sum clamped.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Scene {
    pub fov: f32,
    pub ambient: [f32; 3],
    pub color: [f32; 3],
    pub dir: [f32; 3],
    /// Seconds, for surfaces that move (Tri::shimmer); 0 draws a still frame.
    pub time: f32,
}

impl Default for Scene {
    /// A ViewportFrame's defaults: Ambient (200, 200, 200), LightColor (140, 140, 140), LightDirection (-1, -1, -1),
    /// seen orthographically.
    fn default() -> Self {
        Self {
            fov: 0.0,
            ambient: [200.0 / 255.0; 3],
            color: [140.0 / 255.0; 3],
            dir: [-1.0, -1.0, -1.0],
            time: 0.0,
        }
    }
}

const SS: usize = 3; // samples per pixel along each axis, box-filtered down for smooth edges
// shortcut: a guess fitted to one live case (an Enchant Relic's Neon rune sits 2.5% of the relic's size under its
// stone's face, yet the game shows it); measure again if glowing parts show through where the game hides them
const GLOW_DEPTH_TOLERANCE: f32 = 0.04; // of the model's size: how far under a surface a glow surface still shows
const GLOW_AMBIENT: f32 = 0.7; // Neon: mostly its own light, a little shape kept so it doesn't read as a flat silhouette
const MARGIN: f32 = 0.95; // of the slot the model's extent fills
const HALO_RADIUS: f32 = 0.05; // of the buffer's smaller side
const HALO: f32 = 1.2; // the blur is about half the color at a silhouette, so the halo starts at ~0.6 of it
const GLOW_BIT: u32 = 1 << 24; // marks a sample's packed color as a glow surface
const SHININESS: i32 = 24; // the glint's tightness (Blinn-Phong exponent)
const SPECULAR: f32 = 0.6; // a full-shine glint's brightness, added to the lit color
const AURA: f32 = 0.9; // an aura's strength at the silhouette
const AURA_INSIDE: f32 = 12.0; // from alpha 1/12 a pixel is inside the model (a ForceField body is ~1/5): no aura

/// sRGB 0..1 to linear light (gamma 2.2, close enough for shading).
fn to_linear(c: f32) -> f32 {
    c.max(0.0).powf(2.2)
}

fn to_srgb(c: f32) -> f32 {
    c.max(0.0).powf(1.0 / 2.2)
}

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
/// projected by `scene`'s camera, then scaled and centered so the model's projected extent fills the buffer. Each
/// surface is lit on the side facing the camera (meshes come wound either way).
pub fn render(tris: &[Tri], textures: &[Texture], scene: &Scene, w: u32, h: u32) -> Vec<u8> {
    let (w, h) = (w as usize, h as usize);
    let mut out = vec![0u8; w * h * 4];
    let persp = scene.fov > 1e-3 && scene.fov < 3.0;
    let focal = if persp { 1.0 / (scene.fov / 2.0).tan() } else { 1.0 };
    // x, y on the image plane (y up) and q, larger nearer: 1/distance for perspective, z for orthographic
    let project = |v: [f32; 3]| -> Option<[f32; 3]> {
        if !v.iter().all(|c| c.is_finite()) {
            return None;
        }
        if !persp {
            return Some(v);
        }
        let d = -v[2];
        (d > 1e-4).then(|| [v[0] * focal / d, v[1] * focal / d, 1.0 / d])
    };
    let projected: Vec<Option<[[f32; 3]; 3]>> = tris
        .iter()
        .map(|t| {
            let [a, b, c] = t.p.map(project);
            Some([a?, b?, c?])
        })
        .collect();
    let (mut lo, mut hi) = ([f32::MAX; 2], [f32::MIN; 2]);
    for q in projected.iter().flatten().flatten() {
        for k in 0..2 {
            lo[k] = lo[k].min(q[k]);
            hi[k] = hi[k].max(q[k]);
        }
    }
    let (ex, ey) = (hi[0] - lo[0], hi[1] - lo[1]);
    if w == 0 || h == 0 || !(ex > 0.0 || ey > 0.0) || !ex.is_finite() || !ey.is_finite() {
        return out;
    }
    let (sw, sh) = (w * SS, h * SS);
    // the model's size in camera space, for the glow's depth tolerance
    let (mut plo, mut phi) = ([f32::MAX; 3], [f32::MIN; 3]);
    for v in tris
        .iter()
        .flat_map(|t| t.p)
        .filter(|v| v.iter().all(|c| c.is_finite()))
    {
        for k in 0..3 {
            plo[k] = plo[k].min(v[k]);
            phi[k] = phi[k].max(v[k]);
        }
    }
    let glow_tolerance = GLOW_DEPTH_TOLERANCE * (0..3).map(|k| phi[k] - plo[k]).fold(0.0, f32::max);
    // whether depth q is in front of what is drawn: q is 1/distance under perspective, z (toward the camera) otherwise
    let in_front = |q: f32, drawn: f32, glow: bool| {
        if !glow || drawn == f32::NEG_INFINITY {
            q > drawn
        } else if persp {
            1.0 / q < 1.0 / drawn + glow_tolerance
        } else {
            q > drawn - glow_tolerance
        }
    };
    let scale = MARGIN * (sw as f32 / ex.max(1e-9)).min(sh as f32 / ey.max(1e-9));
    let (cx, cy) = ((lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0);
    let to_samples = |q: [f32; 3]| {
        [
            sw as f32 / 2.0 + (q[0] - cx) * scale,
            sh as f32 / 2.0 - (q[1] - cy) * scale,
            q[2],
        ]
    };
    let toward_light = unit(scene.dir.map(|v| -v)).unwrap_or([0.0, 0.0, 1.0]);
    let mut depth = vec![f32::NEG_INFINITY; sw * sh];
    let mut color = vec![0u32; sw * sh];
    let mut cover = vec![0u8; sw * sh]; // 255 under a solid surface, less under see-through ones only
    let mut any_glow = false;
    // solid first: a see-through surface blends over whatever is behind it, so that must be drawn already
    let order = (0..tris.len())
        .filter(|&i| tris[i].clear <= 0.0)
        .chain((0..tris.len()).filter(|&i| tris[i].clear > 0.0));
    for (t, q) in order.map(|i| (&tris[i], &projected[i])) {
        let Some(q) = q else { continue };
        let [a, b, d] = q.map(to_samples);
        let area = (b[0] - a[0]) * (d[1] - a[1]) - (b[1] - a[1]) * (d[0] - a[0]);
        if area.abs() < 1e-6 {
            continue; // degenerate, or edge-on
        }
        any_glow |= t.glow;
        // the face normal turned toward the camera: the side we see is the side that is lit
        let (u, v) = (
            [0, 1, 2].map(|k| t.p[1][k] - t.p[0][k]),
            [0, 1, 2].map(|k| t.p[2][k] - t.p[0][k]),
        );
        let face = unit([
            u[1] * v[2] - u[2] * v[1],
            u[2] * v[0] - u[0] * v[2],
            u[0] * v[1] - u[1] * v[0],
        ])
        .unwrap_or([0.0, 0.0, 1.0]);
        let eye = if persp {
            let c = [0, 1, 2].map(|k| -(t.p[0][k] + t.p[1][k] + t.p[2][k]) / 3.0);
            unit(c).unwrap_or([0.0, 0.0, 1.0])
        } else {
            [0.0, 0.0, 1.0]
        };
        let face = if dot(face, eye) < 0.0 { face.map(|c| -c) } else { face };
        // given normals point outward; on a surface seen from inside (a thin fin's back) they face away: flip them
        let [n0, n1, n2] = t.n.map(|n| match unit(n) {
            Some(n) if dot(n, face) < 0.0 => n.map(|c| -c),
            Some(n) => n,
            None => face,
        });
        let tex = textures.get((t.tex as usize).wrapping_sub(1));
        let base = [16, 8, 0].map(|s| ((t.rgb >> s) & 0xff) as f32 / 255.0);
        let corner_tint = t.vc.map(|c| {
            if c[3] == 0 {
                [1.0; 3]
            } else {
                [0, 1, 2].map(|k| f32::from(c[k]) / 255.0)
            }
        });
        let glint = unit([0, 1, 2].map(|k| toward_light[k] + eye[k])).unwrap_or(eye); // the Blinn-Phong half vector
        let solid = t.clear <= 0.0;
        let opacity = (1.0 - t.clear).clamp(0.0, 1.0);
        let flag = if t.glow { GLOW_BIT } else { 0 };
        let qs = if persp { [a[2], b[2], d[2]] } else { [1.0; 3] };
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
                // q interpolates linearly on screen (1/distance under perspective): it is the depth test
                let z = w0 * a[2] + w1 * b[2] + w2 * d[2];
                let i = y * sw + x;
                if !in_front(z, depth[i], t.glow) {
                    continue;
                }
                // a ForceField's ripple: a fine, faint speckle drifting with time (as the game's: no broad bands)
                let opacity = if t.shimmer {
                    let (fx, fy) = (x as f32 / SS as f32, y as f32 / SS as f32);
                    let speckle = (fx * 1.7 + scene.time * 2.0).sin() * (fy * 1.3 - scene.time * 1.6).sin();
                    opacity * (0.7 + 0.3 * speckle)
                } else {
                    opacity
                };
                if !solid && opacity < 0.004 {
                    continue;
                }
                if solid {
                    depth[i] = z;
                }
                // perspective-correct weights for the attributes
                let pw = [w0 * qs[0], w1 * qs[1], w2 * qs[2]];
                let sum = pw[0] + pw[1] + pw[2];
                let pw = pw.map(|v| v / sum);
                let n = [0, 1, 2].map(|k| pw[0] * n0[k] + pw[1] * n1[k] + pw[2] * n2[k]);
                let diffuse = (dot(n, toward_light) / dot(n, n).sqrt()).max(0.0);
                let diffuse = if diffuse.is_finite() { diffuse.min(1.0) } else { 0.0 };
                let tint = [0, 1, 2]
                    .map(|k| pw[0] * corner_tint[0][k] + pw[1] * corner_tint[1][k] + pw[2] * corner_tint[2][k]);
                let base = [0, 1, 2].map(|k| base[k] * tint[k]);
                let albedo = match tex {
                    Some(tex) => {
                        let uv = [0, 1].map(|k| pw[0] * t.uv[0][k] + pw[1] * t.uv[1][k] + pw[2] * t.uv[2][k]);
                        let s = tex.sample(uv[0], uv[1]);
                        [0, 1, 2].map(|k| base[k] + (s[k] - base[k]) * s[3])
                    }
                    None => base,
                };
                let spec = if t.shine > 0.0 {
                    let h = (dot(n, glint) / dot(n, n).sqrt()).max(0.0);
                    if h.is_finite() {
                        t.shine * SPECULAR * h.powi(SHININESS)
                    } else {
                        0.0
                    }
                } else {
                    0.0
                };
                let rgb = [0, 1, 2].map(|k| {
                    let v = if t.glow {
                        let v = albedo[k] * (GLOW_AMBIENT + (1.0 - GLOW_AMBIENT) * diffuse);
                        v + (1.0 - v) * 0.3
                    } else {
                        // lit in linear light, as Roblox does: dark colors stay dark under a bright ambient
                        let light = to_linear(scene.ambient[k]) + to_linear(scene.color[k]) * diffuse;
                        to_srgb(to_linear(albedo[k]) * light + spec * (0.5 + to_linear(scene.color[k])))
                    };
                    (v * 255.0).round().clamp(0.0, 255.0) as u32
                });
                let packed = flag | (rgb[0] << 16) | (rgb[1] << 8) | rgb[2];
                if solid {
                    color[i] = packed;
                    cover[i] = 255;
                } else if cover[i] == 0 {
                    color[i] = packed;
                    cover[i] = (opacity * 255.0).round().max(1.0) as u8;
                } else {
                    let mix = |s: u32| {
                        let (a, b) = (((color[i] >> s) & 0xff) as f32, ((packed >> s) & 0xff) as f32);
                        ((a + (b - a) * opacity).round() as u32) << s
                    };
                    color[i] = (color[i] & GLOW_BIT) | flag | mix(16) | mix(8) | mix(0);
                    cover[i] += (f32::from(255 - cover[i]) * opacity) as u8;
                }
            }
        }
    }
    let n = (SS * SS) as u32;
    let mut glow = vec![[0.0f32; 3]; if any_glow { w * h } else { 0 }];
    for y in 0..h {
        for x in 0..w {
            let (mut sum, mut lit, mut hits) = ([0u32; 3], [0u32; 3], 0u32); // hits: summed cover, 255 a sample
            for sy in 0..SS {
                for sx in 0..SS {
                    let i = (y * SS + sy) * sw + x * SS + sx;
                    if cover[i] > 0 {
                        let c = u32::from(cover[i]);
                        hits += c;
                        let ch =
                            [color[i] & 0xff, (color[i] >> 8) & 0xff, (color[i] >> 16) & 0xff].map(|v| v * c / 255);
                        (0..3).for_each(|k| sum[k] += ch[k]);
                        if color[i] & GLOW_BIT != 0 {
                            (0..3).for_each(|k| lit[k] += ch[k]);
                        }
                    }
                }
            }
            // uncovered samples count as transparent black: the averages are already premultiplied
            let o = (y * w + x) * 4;
            out[o..o + 4].copy_from_slice(&[sum[0] / n, sum[1] / n, sum[2] / n, hits / n].map(|v| v as u8));
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

/// A soft glow of color 0xRRGGBB around what a render drew (its alpha), laid behind it in place: the picture keeps
/// its edges and the glow fills the clear space near them, fading out over about a tenth of the smaller side.
pub fn aura(bgra: &mut [u8], w: u32, h: u32, rgb: u32) {
    let (w, h) = (w as usize, h as usize);
    if w == 0 || h == 0 || bgra.len() < w * h * 4 {
        return;
    }
    let mut mask: Vec<[f32; 3]> = bgra
        .chunks(4)
        .take(w * h)
        .map(|p| [f32::from(p[3]), 0.0, 0.0])
        .collect();
    let r = ((w.min(h) as f32 * 0.1 / 2.0).round() as usize).max(1);
    box_blur(&mut mask, w, h, r);
    box_blur(&mut mask, w, h, r);
    let c = [rgb & 0xff, (rgb >> 8) & 0xff, (rgb >> 16) & 0xff].map(|v| v as f32 / 255.0); // b, g, r
    for (px, m) in bgra.as_chunks_mut::<4>().0.iter_mut().zip(&mask) {
        let ha = (m[0] / 255.0 * AURA * 1.6).min(AURA); // ~half at the silhouette after the blur: lift it back
        // only around the model: a see-through body's faint alpha would let it fill the body, a blur over the fish
        let behind = (1.0 - f32::from(px[3]) / 255.0 * AURA_INSIDE).max(0.0);
        for k in 0..3 {
            px[k] += (c[k] * ha * 255.0 * behind) as u8;
        }
        px[3] += (ha * 255.0 * behind) as u8;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tri(p: [[f32; 3]; 3], rgb: u32, glow: bool) -> Tri {
        Tri {
            p,
            rgb,
            glow,
            ..Tri::default()
        }
    }

    fn big(glow: bool) -> Tri {
        tri([[-1.0, -1.0, 0.0], [1.0, -1.0, 0.0], [0.0, 1.0, 0.0]], 0xff8000, glow)
    }

    fn r(tris: &[Tri], w: u32, h: u32) -> Vec<u8> {
        render(tris, &[], &Scene::default(), w, h)
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
        let b = r(&[big(false)], 40, 40);
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
        assert_eq!(r(&[], 12, 7), vec![0; 12 * 7 * 4]);
        assert!(r(&[big(false)], 0, 7).is_empty());
        let junk = [
            tri([[0.0; 3]; 3], 0xffffff, false),
            tri(
                [[f32::NAN, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
                0xffffff,
                false,
            ),
        ];
        assert_eq!(r(&junk, 12, 7), vec![0; 12 * 7 * 4]);
        // a NaN triangle beside a real one is skipped, the real one still drawn
        assert!(r(&[junk[1], big(false)], 12, 7).chunks(4).any(|p| p[3] == 255));
    }

    #[test]
    fn stays_inside_the_buffer() {
        let b = r(&boxed(), 120, 60);
        for x in 0..120 {
            assert_eq!(px(&b, 120, x, 0)[3], 0);
            assert_eq!(px(&b, 120, x, 59)[3], 0);
        }
        assert!(b.chunks(4).filter(|p| p[3] > 128).count() > 1000);
    }

    #[test]
    fn perspective_makes_the_near_end_bigger() {
        // a long flat strip running away from the camera: its near end spans more pixels than its far end
        let strip = [
            tri(
                [[-1.0, 0.0, -2.0], [1.0, 0.0, -2.0], [1.0, 0.0, -12.0]],
                0xffffff,
                false,
            ),
            tri(
                [[1.0, 0.0, -12.0], [-1.0, 0.0, -12.0], [-1.0, 0.0, -2.0]],
                0xffffff,
                false,
            ),
        ]
        .map(|mut t| {
            t.p = t.p.map(|v| [v[0], v[1] - 1.0 + (v[2] + 2.0) * -0.15, v[2]]); // tilted so it shows
            t
        });
        let scene = Scene {
            fov: 1.2,
            ..Scene::default()
        };
        let b = render(&strip, &[], &scene, 60, 60);
        let width = |y: u32| (0..60).filter(|&x| px(&b, 60, x, y)[3] > 128).count();
        let rows: Vec<u32> = (0..60).filter(|&y| width(y) > 0).collect();
        let (top, bottom) = (rows[1], rows[rows.len() - 2]);
        assert!(width(bottom) > width(top) * 2, "{} {}", width(bottom), width(top));
        // nothing behind the camera is drawn
        let behind = tri([[-1.0, -1.0, 1.0], [1.0, -1.0, 1.0], [0.0, 1.0, 1.0]], 0xffffff, false);
        assert_eq!(render(&[behind], &[], &scene, 10, 10), vec![0; 400]);
    }

    #[test]
    fn see_through_shows_what_is_behind() {
        // a white see-through triangle in front of a blue solid one: bluish, lighter; alone: faint
        let front = Tri {
            clear: 0.65,
            ..tri([[-1.0, -1.0, 1.0], [1.0, -1.0, 1.0], [0.0, 1.0, 1.0]], 0xffffff, false)
        };
        let back = tri([[-1.0, -1.0, 0.0], [1.0, -1.0, 0.0], [0.0, 1.0, 0.0]], 0x0000ff, false);
        let both = r(&[front, back], 40, 40); // listed first, still drawn over
        let [b, g, red, a] = px(&both, 40, 20, 22);
        assert!(
            a == 255 && b > 200 && red > 60 && red < 160 && g == red,
            "{red} {g} {b} {a}"
        );
        let alone = r(&[front], 40, 40);
        let [_, _, _, a] = px(&alone, 40, 20, 22);
        assert!(a > 60 && a < 120, "{a}");
        assert!(premultiplied(&alone));
    }

    #[test]
    fn glow_shows_through_a_hair_thin_cover() {
        // a glowing triangle 2% of the model's size behind a solid one still shows; 20% behind it doesn't
        let cover = tri([[-1.0, -1.0, 0.0], [1.0, -1.0, 0.0], [0.0, 1.0, 0.0]], 0x404040, false);
        let rune = |depth: f32| {
            tri(
                [[-0.3, -0.3, -depth], [0.3, -0.3, -depth], [0.0, 0.3, -depth]],
                0x00ff00,
                true,
            )
        };
        let green = |depth| px(&r(&[cover, rune(depth)], 40, 40), 40, 20, 20)[1];
        assert!(green(0.04) > 200, "{}", green(0.04));
        assert!(green(0.4) < 100, "{}", green(0.4));
    }

    #[test]
    fn vertex_colors_tint_the_color() {
        let mut t = big(false);
        t.rgb = 0xffffff;
        t.vc = [[255, 0, 0, 255]; 3];
        let [b, g, red, _] = px(&r(&[t], 30, 30), 30, 15, 16);
        assert!(red > 150 && g < 10 && b < 10, "{red} {g} {b}");
        t.vc = [[0; 4]; 3]; // none given: white
        let [b, g, red, _] = px(&r(&[t], 30, 30), 30, 15, 16);
        assert!(red > 150 && red == g && g == b, "{red} {g} {b}");
    }

    #[test]
    fn shine_adds_a_glint() {
        // a ball lit from the front: the glossy one is brighter where the light reflects at the camera
        let dull = sphere(true);
        let glossy: Vec<Tri> = dull.iter().map(|t| Tri { shine: 1.0, ..*t }).collect();
        let scene = Scene {
            dir: [0.0, 0.0, -1.0],
            ambient: [0.2; 3],
            color: [0.4; 3], // dim: the default light already whitens a light grey
            ..Scene::default()
        };
        let peak = |m: &[Tri]| {
            render(m, &[], &scene, 60, 60)
                .chunks(4)
                .map(|p| u32::from(p[0]) + u32::from(p[1]) + u32::from(p[2]))
                .max()
                .unwrap()
        };
        assert!(peak(&glossy) > peak(&dull) + 60, "{} {}", peak(&glossy), peak(&dull));
    }

    #[test]
    fn shimmer_moves_with_time() {
        let t = Tri {
            clear: 0.5,
            shimmer: true,
            ..big(false)
        };
        let at = |time| {
            let scene = Scene {
                time,
                ..Scene::default()
            };
            render(&[t], &[], &scene, 40, 40)
        };
        let (a, b) = (at(0.0), at(0.5));
        assert!(a != b && premultiplied(&a) && premultiplied(&b));
        assert!(a.chunks(4).all(|p| p[3] < 255)); // never solid
    }

    #[test]
    fn aura_glows_around_the_model_only() {
        let mut b = r(&[big(false)], 60, 60);
        let before = b.clone();
        aura(&mut b, 60, 60, 0x00ff00);
        assert!(premultiplied(&b));
        // the clear space by the edge turns green; the model and far corners stay as they were
        let grew = before
            .chunks(4)
            .zip(b.chunks(4))
            .filter(|(o, n)| o[3] == 0 && n[3] > 0 && n[1] > n[2])
            .count();
        assert!(grew > 20, "{grew}");
        assert_eq!(px(&b, 60, 30, 32), px(&before, 60, 30, 32));
        assert_eq!(px(&b, 60, 0, 0), [0; 4]);
    }

    #[test]
    fn aura_leaves_a_see_through_body_clear() {
        // a ghostly body (alpha ~1/5) must not fill with the glow: it would read as a blur over the fish
        let mut b = vec![0u8; 60 * 60 * 4];
        for y in 15..45 {
            for x in 15..45 {
                b[(y * 60 + x) * 4..][..4].copy_from_slice(&[10, 10, 10, 50]);
            }
        }
        aura(&mut b, 60, 60, 0x00ff00);
        assert_eq!(px(&b, 60, 30, 30), [10, 10, 10, 50]);
        assert!(px(&b, 60, 30, 12)[1] > 0); // still glows just outside
    }

    #[test]
    fn texture_lays_over_the_color_by_its_alpha() {
        // left half opaque red, right half clear: the clear half shows the triangle's own blue
        let mut rgba = Vec::new();
        for _ in 0..4 {
            rgba.extend([255, 0, 0, 255, 255, 0, 0, 255, 0, 255, 0, 0, 0, 255, 0, 0]);
        }
        let tex = Texture { w: 4, h: 4, rgba };
        let quad = [
            (
                [[-1.0, -1.0, 0.0], [1.0, -1.0, 0.0], [1.0, 1.0, 0.0]],
                [[0.0, 1.0], [1.0, 1.0], [1.0, 0.0]],
            ),
            (
                [[1.0, 1.0, 0.0], [-1.0, 1.0, 0.0], [-1.0, -1.0, 0.0]],
                [[1.0, 0.0], [0.0, 0.0], [0.0, 1.0]],
            ),
        ]
        .map(|(p, uv)| Tri {
            uv,
            tex: 1,
            ..tri(p, 0x0000ff, false)
        });
        let b = render(&quad, &[tex], &Scene::default(), 40, 40);
        let [bl, g, red, _] = px(&b, 40, 8, 20);
        assert!(red > 200 && g < 20 && bl < 20, "{red} {g} {bl}");
        let [bl, g, red, _] = px(&b, 40, 31, 20);
        assert!(bl > 200 && g < 20 && red < 20, "{red} {g} {bl}");
    }

    #[test]
    fn glow_is_brighter_than_lit() {
        let sum = |t| {
            px(&r(&[t], 20, 20), 20, 10, 10)[..3]
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
            let b = r(model, 80, 80);
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
        let flat = r(&[big(false)], 30, 30);
        let close = |b: Vec<u8>| flat.iter().zip(&b).all(|(x, y)| x.abs_diff(*y) <= 1);
        for n in [
            [[0.0, 0.0, 1.0]; 3],
            [[0.0, 0.0, -2.0]; 3],                             // either side, any length
            [[0.0, 0.0, 1.0], [0.0; 3], [f32::NAN, 0.0, 0.0]], // given where known, the face elsewhere
        ] {
            assert!(close(r(&[Tri { n, ..big(false) }], 30, 30)), "{n:?}");
        }
    }

    #[test]
    fn glow_has_a_halo() {
        let (lit, glow) = (r(&[big(false)], 60, 60), r(&[big(true)], 60, 60));
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
