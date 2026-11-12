use super::memory::{Memory, Pod};
use super::offsets::Offsets;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vec2 {
    pub x: f32,
    pub y: f32,
}
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}
unsafe impl Pod for Vec2 {}
unsafe impl Pod for Vec3 {}

pub const fn vec3(x: f32, y: f32, z: f32) -> Vec3 {
    Vec3 { x, y, z }
}

pub fn distance(a: Vec3, b: Vec3) -> f32 {
    let (x, y, z) = (a.x - b.x, a.y - b.y, a.z - b.z);
    (x * x + y * y + z * z).sqrt()
}

/// What the VisualEngine renders with: row-major clip = m * (x, y, z, 1), and the render size in client pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ScreenView {
    pub m: [f32; 16],
    pub size: Vec2,
}

pub const SCREEN_KEYS: [&str; 3] = [
    "VisualEngine.Pointer",
    "VisualEngine.ViewMatrix",
    "VisualEngine.Dimensions",
];

/// `base` is the Roblox module base (`RobloxProcess::base()`). None when unreadable or the size is not positive.
pub fn read_screen_view(m: &dyn Memory, o: &Offsets, base: usize) -> Option<ScreenView> {
    let ve = m
        .get::<usize>(o.at(base, "VisualEngine.Pointer"))
        .filter(|ve| *ve != 0)?;
    let mat = m.get::<[f32; 16]>(o.at(ve, "VisualEngine.ViewMatrix"))?;
    let size = m.get::<Vec2>(o.at(ve, "VisualEngine.Dimensions"))?;
    (size.x > 0.0 && size.y > 0.0).then_some(ScreenView { m: mat, size })
}

/// Client-area pixel of a world point; None behind (or almost at) the camera. May lie outside the client area.
pub fn world_to_screen(v: &ScreenView, p: Vec3) -> Option<Vec2> {
    let m = &v.m;
    let w = p.x * m[12] + p.y * m[13] + p.z * m[14] + m[15];
    if w.is_nan() || w <= 0.1 {
        return None; // behind the camera
    }
    let x = (p.x * m[0] + p.y * m[1] + p.z * m[2] + m[3]) / w;
    let y = (p.x * m[4] + p.y * m[5] + p.z * m[6] + m[7]) / w;
    Some(Vec2 {
        x: (x + 1.0) * v.size.x / 2.0,
        y: (1.0 - y) * v.size.y / 2.0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::FakeMemory;

    // Looks down -z with x right, y up; w = -z, so a point 10 studs ahead has w = 10.
    fn forward() -> ScreenView {
        ScreenView {
            m: [1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., -1., 0.],
            size: Vec2 { x: 800., y: 600. },
        }
    }

    #[test]
    fn screen_projection() {
        assert_eq!(
            world_to_screen(&forward(), vec3(0., 0., -10.)),
            Some(Vec2 { x: 400., y: 300. })
        );
        assert_eq!(
            world_to_screen(&forward(), vec3(5., 5., -10.)),
            Some(Vec2 { x: 600., y: 150. })
        ); // up = smaller y
        assert_eq!(world_to_screen(&forward(), vec3(0., 0., 10.)), None);
        assert_eq!(world_to_screen(&forward(), vec3(0., 0., 0.)), None);
        assert_eq!(distance(vec3(0., 0., 0.), vec3(3., 4., 0.)), 5.0);
    }

    #[test]
    fn screen_read_view() {
        let mut m = FakeMemory::default();
        let o = Offsets::from_pairs(&[
            ("VisualEngine.Pointer", 0x100),
            ("VisualEngine.ViewMatrix", 0x10),
            ("VisualEngine.Dimensions", 0x80),
        ]);
        let (base, ve) = (0x1000, m.alloc(0x100));
        m.put_ptr(base + 0x100, ve);
        let want = forward();
        m.put_val(ve + 0x10, &want.m);
        m.put_val(ve + 0x80, &want.size);
        assert_eq!(read_screen_view(&m, &o, base), Some(want));

        m.put_val(ve + 0x80, &Vec2 { x: 0., y: 600. });
        assert_eq!(read_screen_view(&m, &o, base), None); // minimised window: no render size
        m.put_ptr(base + 0x100, 0);
        assert_eq!(read_screen_view(&m, &o, base), None);
    }
}
