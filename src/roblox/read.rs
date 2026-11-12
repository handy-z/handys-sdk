//! Typed reads of the Roblox objects every game touches. Each takes an instance address and gives 0 / None when it
//! is 0 or the read fails, so chains need no checks in between. The comment names the offset keys each one reads
//! (besides those of SDK_KEYS); indexing `Offsets` panics when one is missing from the game's required list.
use super::instance::{child_by_class, children, instance_class, instance_name, read_string};
use super::memory::{Memory, Pod};
use super::offsets::{OffsetKeys, Offsets};
use super::screen::{Vec2, Vec3};
use std::time::{Duration, Instant};

/// Row-major 3x3 rotation, as Roblox stores a CFrame's.
pub type Mat3 = [f32; 9];

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Color3 {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct UDim2 {
    pub x_scale: f32,
    pub x_offset: i32,
    pub y_scale: f32,
    pub y_offset: i32,
}

/// Pixels from the client area's top-left.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub pos: Vec2,
    pub size: Vec2,
}

unsafe impl Pod for Color3 {}
unsafe impl Pod for UDim2 {}
unsafe impl Pod for Rect {}
const _: () = assert!(
    size_of::<Color3>() == 3 && size_of::<UDim2>() == 16 && size_of::<Rect>() == 16 && size_of::<Mat3>() == 36,
    "the structs mirror Roblox's memory layout"
);

/// Offset keys a group of reads below needs, to add to a game's required list.
#[cfg(test)]
pub(crate) const PART_KEYS: [&str; 2] = ["BasePart.Primitive", "Primitive.Position"];
#[cfg(test)]
pub(crate) const LOCAL_ROOT_KEYS: [&str; 5] = [
    "Player.LocalPlayer",
    "Player.ModelInstance",
    "Model.PrimaryPart",
    "BasePart.Primitive",
    "Primitive.Position",
];

/// `groups` then `extra`, each key once, in first-seen order: `join_keys(&[&SDK_KEYS, &SCREEN_KEYS], &["DataModel.GameId"])`.
pub fn join_keys(groups: &[&[&'static str]], extra: &[&'static str]) -> OffsetKeys {
    let mut out = OffsetKeys::new();
    for k in groups.iter().flat_map(|g| g.iter()).chain(extra) {
        if !out.contains(k) {
            out.push(k);
        }
    }
    out
}

/// The pointer stored at `addr`; 0 when addr is 0 or unreadable.
pub fn ptr_at(m: &dyn Memory, addr: usize) -> usize {
    if addr == 0 {
        0
    } else {
        m.get::<usize>(addr).unwrap_or(0)
    }
}

fn at<T: Pod>(m: &dyn Memory, o: &Offsets, inst: usize, key: &str) -> Option<T> {
    if inst == 0 { None } else { m.get::<T>(o.at(inst, key)) }
}

fn ptr_field(m: &dyn Memory, o: &Offsets, inst: usize, key: &str) -> usize {
    if inst == 0 { 0 } else { ptr_at(m, o.at(inst, key)) }
}

// Tree. children()/child_by_name()/resolve_path() and child_count() are in instance.rs.

/// Instance.Parent
pub fn parent_of(m: &dyn Memory, o: &Offsets, inst: usize) -> usize {
    ptr_field(m, o, inst, "Instance.Parent")
}

/// resolve_path that remembers what it found. A later get() checks, from the root down, that each node it has is
/// still among its parent's children (one bulk read per level, no name reads; a freed instance keeps its stale Parent
/// pointer but leaves its parent's list) and walks names again only below the first level that broke. While a level
/// is missing it reads that parent's names again only when its children changed. A new root starts over. One per
/// reader.
#[derive(Clone, Debug, Default)]
pub struct CachedPath {
    names: Vec<String>,
    root: usize,
    nodes: Vec<usize>,     // nodes[i] is the child names[i] of nodes[i - 1] (of root for 0)
    missed_in: Vec<usize>, // children of the parent where names[nodes.len()] was not found
}

impl CachedPath {
    pub fn new(names: &[&str]) -> Self {
        Self {
            names: names.iter().map(|n| (*n).to_owned()).collect(),
            ..Self::default()
        }
    }

    pub fn names(&self) -> &[String] {
        &self.names
    }

    /// 0 while any level is missing.
    pub fn get(&mut self, m: &dyn Memory, o: &Offsets, root: usize) -> usize {
        if root != self.root {
            self.root = root;
            self.nodes.clear();
            self.missed_in.clear();
        }
        if root == 0 {
            return 0;
        }
        for i in 0..self.nodes.len() {
            let kids = children(m, o, if i > 0 { self.nodes[i - 1] } else { root });
            if !kids.contains(&self.nodes[i]) {
                self.nodes.truncate(i);
                self.missed_in.clear();
                break;
            }
        }
        while self.nodes.len() < self.names.len() {
            let parent = self.nodes.last().copied().unwrap_or(root);
            let kids = children(m, o, parent);
            if !self.missed_in.is_empty() && kids == self.missed_in {
                return 0; // same children as when it was missing
            }
            let want = &self.names[self.nodes.len()];
            let Some(c) = kids
                .iter()
                .copied()
                .find(|k| instance_name(m, o, *k).as_ref() == Some(want))
            else {
                self.missed_in = kids;
                return 0;
            };
            self.missed_in.clear();
            self.nodes.push(c);
        }
        self.nodes.last().copied().unwrap_or(root)
    }
}

// World. `dm` is the DataModel (instance.rs data_model()), `players` its Players child (child_by_class(.., "Players")).

/// DataModel.Workspace
pub fn workspace_of(m: &dyn Memory, o: &Offsets, dm: usize) -> usize {
    ptr_field(m, o, dm, "DataModel.Workspace")
}
/// + Workspace.CurrentCamera
pub fn current_camera(m: &dyn Memory, o: &Offsets, dm: usize) -> usize {
    ptr_field(m, o, workspace_of(m, o, dm), "Workspace.CurrentCamera")
}
/// Camera.Position
pub fn camera_position(m: &dyn Memory, o: &Offsets, cam: usize) -> Option<Vec3> {
    at(m, o, cam, "Camera.Position")
}
/// Camera.Rotation
pub fn camera_rotation(m: &dyn Memory, o: &Offsets, cam: usize) -> Option<Mat3> {
    at(m, o, cam, "Camera.Rotation")
}
/// Camera.CameraSubject
pub fn camera_subject(m: &dyn Memory, o: &Offsets, cam: usize) -> usize {
    ptr_field(m, o, cam, "Camera.CameraSubject")
}

// Players.

/// Player.LocalPlayer
pub fn local_player(m: &dyn Memory, o: &Offsets, players: usize) -> usize {
    ptr_field(m, o, players, "Player.LocalPlayer")
}
/// Player.ModelInstance
pub fn character_of(m: &dyn Memory, o: &Offsets, player: usize) -> usize {
    ptr_field(m, o, player, "Player.ModelInstance")
}
/// Its PlayerGui child.
pub fn player_gui(m: &dyn Memory, o: &Offsets, player: usize) -> usize {
    if player == 0 {
        0
    } else {
        child_by_class(m, o, player, "PlayerGui")
    }
}
/// Position of the local character's PrimaryPart (its HumanoidRootPart); LOCAL_ROOT_KEYS.
pub fn local_root_position(m: &dyn Memory, o: &Offsets, players: usize) -> Option<Vec3> {
    part_position(
        m,
        o,
        primary_part(m, o, character_of(m, o, local_player(m, o, players))),
    )
}

// Parts and models.

/// Classes that derive from BasePart, so they carry BasePart.Primitive. Terrain is one too but spans the map.
pub(crate) const BASE_PART_CLASSES: [&str; 13] = [
    "Part",
    "MeshPart",
    "UnionOperation",
    "WedgePart",
    "CornerWedgePart",
    "TrussPart",
    "SpawnLocation",
    "Seat",
    "VehicleSeat",
    "SkateboardPlatform",
    "PartOperation",
    "NegateOperation",
    "IntersectOperation",
];

pub fn is_base_part(class_name: &str) -> bool {
    BASE_PART_CLASSES.contains(&class_name)
}
/// Model.PrimaryPart
pub fn primary_part(m: &dyn Memory, o: &Offsets, model: usize) -> usize {
    ptr_field(m, o, model, "Model.PrimaryPart")
}
/// BasePart.Primitive
pub fn primitive_of(m: &dyn Memory, o: &Offsets, part: usize) -> usize {
    ptr_field(m, o, part, "BasePart.Primitive")
}
/// + Primitive.Position
pub fn part_position(m: &dyn Memory, o: &Offsets, part: usize) -> Option<Vec3> {
    at(m, o, primitive_of(m, o, part), "Primitive.Position")
}
/// + Primitive.Size
pub fn part_size(m: &dyn Memory, o: &Offsets, part: usize) -> Option<Vec3> {
    at(m, o, primitive_of(m, o, part), "Primitive.Size")
}
/// + Primitive.Rotation
pub fn part_rotation(m: &dyn Memory, o: &Offsets, part: usize) -> Option<Mat3> {
    at(m, o, primitive_of(m, o, part), "Primitive.Rotation")
}
/// A part's position, or a Model's PrimaryPart's; None for anything else. Reads the class name.
pub fn position_of(m: &dyn Memory, o: &Offsets, inst: usize) -> Option<Vec3> {
    if inst == 0 {
        return None;
    }
    let cls = instance_class(m, o, inst).unwrap_or_default();
    if is_base_part(&cls) {
        part_position(m, o, inst)
    } else if cls == "Model" {
        part_position(m, o, primary_part(m, o, inst))
    } else {
        None
    }
}

// GUI objects (Frame, TextLabel, ImageButton...).

/// GuiObject.Visible == 1 (apps read it through `VisibleProbe`).
#[cfg(test)]
pub(crate) fn gui_visible(m: &dyn Memory, o: &Offsets, gui: usize) -> Option<bool> {
    at::<u8>(m, o, gui, "GuiObject.Visible").map(|v| v == 1)
}

/// Bytes of each anchor searched for its Visible flag.
const VISIBLE_WINDOW: usize = 0x900;

/// The one offset where every `shown` snapshot holds 1, every `hidden` one holds 0, and every `others` one holds 0 or
/// 1 (a flag, not some other field): the real Visible flag. None when no offset or more than one fits (or `shown` or
/// `hidden` is empty): guess nothing.
pub(crate) fn find_visible_offset(shown: &[Vec<u8>], hidden: &[Vec<u8>], others: &[Vec<u8>]) -> Option<usize> {
    let len = shown.iter().chain(hidden).chain(others).map(Vec::len).min()?;
    if shown.is_empty() || hidden.is_empty() {
        return None;
    }
    let mut fits = (0..len).filter(|&i| {
        shown.iter().all(|s| s[i] == 1) && hidden.iter().all(|h| h[i] == 0) && others.iter().all(|x| x[i] <= 1)
    });
    let first = fits.next()?;
    fits.next().is_none().then_some(first)
}

/// `GuiObject.Visible` from the offsets dump was wrong on some Roblox builds (e.g. version-4ea8e85481e34f46: 1837,
/// really 837, read first as 1 on every object, later as 0, 2 or 127). An app that knows GUI objects that are
/// normally shown and ones that are normally hidden calls `calibrate` with them each sample, plus any other GUI
/// objects (`others`) as a sanity check; when the offset in use disagrees with them it searches for the offset that
/// agrees, and `read` uses that from then on. Until one is found `read` uses the dumped offset. The offset in use is
/// checked again twice a second, because one found while the game was loading can be wrong (seen live: the catch XP
/// popup read as hidden until the app restarted). When an anchor is legitimately in the other state no offset fits
/// and nothing changes.
#[derive(Default)]
pub struct VisibleProbe {
    off: Option<usize>,
    last_check: Option<Instant>,
}

impl VisibleProbe {
    pub fn read(&self, m: &dyn Memory, o: &Offsets, gui: usize) -> Option<bool> {
        self.raw(m, o, gui).map(|v| v == 1)
    }

    /// The flag's byte itself. A real Visible flag is only ever 0 or 1, so anything else means the offset is wrong.
    pub fn raw(&self, m: &dyn Memory, o: &Offsets, gui: usize) -> Option<u8> {
        match self.off {
            None => at::<u8>(m, o, gui, "GuiObject.Visible"),
            Some(off) => (gui != 0).then(|| m.get::<u8>(gui.wrapping_add(off))).flatten(),
        }
    }

    /// The current offset gives a real flag on `others`: every readable byte is 0 or 1, and at least one is 1 (a game
    /// screen always shows something). True when none is readable: nothing to judge.
    pub fn plausible(&self, m: &dyn Memory, o: &Offsets, others: &[usize]) -> bool {
        let v: Vec<u8> = others.iter().filter_map(|&g| self.raw(m, o, g)).collect();
        v.is_empty() || (v.iter().all(|&b| b <= 1) && v.contains(&1))
    }

    /// true when it scanned memory for the offset (found or not). `others` (may be empty) are any GUI objects; each
    /// read costs one `Memory::read`, so a caller polling fast passes them only when it has a reason to doubt.
    pub fn calibrate(
        &mut self,
        m: &dyn Memory,
        o: &Offsets,
        shown: &[usize],
        hidden: &[usize],
        others: &[usize],
    ) -> bool {
        if shown.contains(&0) || hidden.contains(&0) {
            return false;
        }
        // checked, and rescanned when wrong, at most twice a second
        if self
            .last_check
            .is_some_and(|t| t.elapsed() <= Duration::from_millis(500))
        {
            return false;
        }
        self.last_check = Some(Instant::now());
        let agrees = shown.iter().all(|&g| self.read(m, o, g) == Some(true))
            && hidden.iter().all(|&g| self.read(m, o, g) == Some(false))
            && self.plausible(m, o, others);
        if agrees {
            return false;
        }
        let snap = |&g: &usize| {
            let mut b = vec![0u8; VISIBLE_WINDOW];
            m.read(g, &mut b).then_some(b)
        };
        let (s, h): (Vec<_>, Vec<_>) = (shown.iter().map(snap).collect(), hidden.iter().map(snap).collect());
        if let (Some(s), Some(h)) = (
            s.into_iter().collect::<Option<Vec<_>>>(),
            h.into_iter().collect::<Option<Vec<_>>>(),
        ) {
            let x: Vec<_> = others.iter().filter(|&&g| g != 0).filter_map(snap).collect();
            // a scan that finds nothing keeps the offset in use: an anchor may just be in its other state
            self.off = find_visible_offset(&s, &h, &x).or(self.off);
        }
        true
    }
}

/// GuiObject.Text
pub fn gui_text(m: &dyn Memory, o: &Offsets, gui: usize) -> Option<String> {
    if gui == 0 {
        None
    } else {
        read_string(m, o, o.at(gui, "GuiObject.Text"))
    }
}
/// GuiObject.Size
pub fn gui_size(m: &dyn Memory, o: &Offsets, gui: usize) -> Option<UDim2> {
    at(m, o, gui, "GuiObject.Size")
}
/// GuiObject.LayoutOrder
pub fn gui_layout_order(m: &dyn Memory, o: &Offsets, gui: usize) -> Option<i32> {
    at(m, o, gui, "GuiObject.LayoutOrder")
}
/// GuiBase2D.AbsolutePosition and the AbsoluteSize stored right after it (seen on version-5a21b87bbc2641cf).
pub fn gui_absolute_rect(m: &dyn Memory, o: &Offsets, gui: usize) -> Option<Rect> {
    at(m, o, gui, "GuiBase2D.AbsolutePosition")
}

// Value objects, all at Misc.Value.

/// NumberValue
pub fn number_value(m: &dyn Memory, o: &Offsets, v: usize) -> Option<f64> {
    at(m, o, v, "Misc.Value")
}
/// IntValue
pub fn int_value(m: &dyn Memory, o: &Offsets, v: usize) -> Option<i64> {
    at(m, o, v, "Misc.Value")
}
/// ObjectValue
pub fn object_value(m: &dyn Memory, o: &Offsets, v: usize) -> usize {
    ptr_field(m, o, v, "Misc.Value")
}
/// StringValue
pub fn string_value(m: &dyn Memory, o: &Offsets, v: usize) -> Option<String> {
    if v == 0 {
        None
    } else {
        read_string(m, o, o.at(v, "Misc.Value"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::roblox::instance::{child_by_name, child_count};
    use crate::roblox::offsets::SDK_KEYS;
    use crate::roblox::screen::vec3;
    use crate::testing::{FakeMemory, fake_instance_offsets};

    // fake_instance_offsets plus fields in the free space past 0x40 of FakeMemory::instance.
    fn offs() -> Offsets {
        let mut o = fake_instance_offsets();
        o.values.extend(
            [
                ("Instance.Parent", 0x40),
                ("Model.PrimaryPart", 0x48),
                ("BasePart.Primitive", 0x50),
                ("Primitive.Position", 0x10),
                ("Primitive.Size", 0x1C),
                ("GuiObject.Visible", 0x58),
                ("GuiObject.Text", 0x60), // an inline MSVC string: 0x60..0x80
                ("Misc.Value", 0x80),
                ("Player.LocalPlayer", 0x88),
                ("Player.ModelInstance", 0x90),
                ("DataModel.Workspace", 0x98),
                ("BasePart.Color3", 0xA0),
                ("GuiObject.Size", 0xB0),
            ]
            .map(|(k, v)| (k.to_owned(), v)),
        );
        o
    }

    // A part whose primitive sits at `pos`.
    fn part(m: &mut FakeMemory, cls: &str, pos: Vec3) -> usize {
        let (p, prim) = (m.instance(cls, "P", &[]), m.alloc(0x40));
        m.put_ptr(p + 0x50, prim);
        m.put_val(prim + 0x10, &pos);
        p
    }

    fn parent(m: &mut FakeMemory, child: usize, to: usize) {
        m.put_ptr(child + 0x40, to);
    }

    #[test]
    fn read_join_keys_dedupes_in_order() {
        let k = join_keys(
            &[&SDK_KEYS, &PART_KEYS, &LOCAL_ROOT_KEYS],
            &["DataModel.GameId", "Primitive.Position"],
        );
        assert_eq!(k.len(), SDK_KEYS.len() + 2 + 3 + 1);
        assert_eq!(k[0], SDK_KEYS[0]);
        assert_eq!(*k.last().unwrap(), "DataModel.GameId");
    }

    #[test]
    fn read_null_and_unreadable_give_nothing() {
        let (mut m, o) = (FakeMemory::default(), offs());
        assert_eq!(ptr_at(&m, 0), 0);
        assert_eq!(ptr_at(&m, 0x999999), 0);
        assert!(part_position(&m, &o, 0).is_none());
        assert!(gui_visible(&m, &o, 0).is_none());
        let label = m.instance("TextLabel", "L", &[]);
        assert!(gui_text(&m, &o, label).is_none()); // no text written
        assert_eq!(local_player(&m, &o, 0), 0);
        assert!(local_root_position(&m, &o, 0).is_none());
    }

    #[test]
    fn read_part_and_model_positions() {
        let (mut m, o) = (FakeMemory::default(), offs());
        let p = part(&mut m, "MeshPart", vec3(1., 2., 3.));
        assert_eq!(part_position(&m, &o, p), Some(vec3(1., 2., 3.)));
        let prim = primitive_of(&m, &o, p);
        m.put_val(prim + 0x1C, &vec3(4., 5., 6.));
        assert_eq!(part_size(&m, &o, p).map(|s| s.z), Some(6.));

        let model = m.instance("Model", "M", &[p]);
        assert!(position_of(&m, &o, model).is_none()); // no PrimaryPart yet
        m.put_ptr(model + 0x48, p);
        assert_eq!(position_of(&m, &o, model).map(|v| v.y), Some(2.));
        assert_eq!(position_of(&m, &o, p).map(|v| v.x), Some(1.));
        let folder = m.instance("Folder", "F", &[]);
        assert!(position_of(&m, &o, folder).is_none());
        assert!(is_base_part("Part") && is_base_part("UnionOperation") && !is_base_part("Model"));
        assert!(!is_base_part("Terrain"));
    }

    #[test]
    fn read_local_root_position() {
        let (mut m, o) = (FakeMemory::default(), offs());
        let root = part(&mut m, "Part", vec3(7., 8., 9.));
        let character = m.instance("Model", "Me", &[root]);
        m.put_ptr(character + 0x48, root);
        let me = m.instance("Player", "Me", &[]);
        let players = m.instance("Players", "Players", &[me]);
        m.put_ptr(players + 0x88, me);
        assert!(local_root_position(&m, &o, players).is_none()); // not spawned
        m.put_ptr(me + 0x90, character);
        assert_eq!(character_of(&m, &o, local_player(&m, &o, players)), character);
        assert_eq!(local_root_position(&m, &o, players).map(|p| p.z), Some(9.));
        assert_eq!(player_gui(&m, &o, me), 0);
    }

    #[test]
    fn find_visible_offset_needs_one_fit() {
        let shown = vec![vec![7, 1, 1], vec![9, 1, 0]];
        let hidden = vec![vec![7, 0, 1]];
        assert_eq!(find_visible_offset(&shown, &hidden, &[]), Some(1));
        assert_eq!(find_visible_offset(&[vec![1, 1]], &[vec![0, 0]], &[]), None); // two fit: ambiguous
        assert_eq!(find_visible_offset(&[vec![1]], &[vec![1]], &[]), None); // nothing fits
        assert_eq!(find_visible_offset(&[], &hidden, &[]), None);
        // other GUI objects rule out a field that is not a flag: offset 0 holds 2 on one of them
        let others = vec![vec![1, 0], vec![2, 1]];
        assert_eq!(find_visible_offset(&[vec![1, 1]], &[vec![0, 0]], &others), Some(1));
    }

    #[test]
    fn visible_probe_doubts_a_flag_that_is_not_a_flag() {
        let (mut m, o) = (FakeMemory::default(), offs());
        let objs: Vec<usize> = (0..4).map(|_| m.alloc(VISIBLE_WINDOW)).collect();
        let p = VisibleProbe::default();
        for (g, v) in objs.iter().zip([0u8, 1, 0, 1]) {
            m.put(*g, &[0; VISIBLE_WINDOW]);
            m.put(*g + 0x58, &[v]);
        }
        assert!(p.plausible(&m, &o, &objs));
        m.put(objs[1] + 0x58, &[2]); // a value a flag never holds
        assert!(!p.plausible(&m, &o, &objs));
        for g in &objs {
            m.put(*g + 0x58, &[0]); // nothing on screen at all
        }
        assert!(!p.plausible(&m, &o, &objs));
        assert!(p.plausible(&m, &o, &[])); // nothing to judge
    }

    #[test]
    fn visible_probe_finds_the_real_flag() {
        let (mut m, o) = (FakeMemory::default(), offs());
        let (a, b, c) = (
            m.alloc(VISIBLE_WINDOW),
            m.alloc(VISIBLE_WINDOW),
            m.alloc(VISIBLE_WINDOW),
        );
        for g in [a, b, c] {
            m.put(g, &[0; VISIBLE_WINDOW]);
        }
        // dumped flag (0x58) says all hidden, the real one at 0x70 says a and b shown, c hidden
        m.put(a + 0x70, &[1]);
        m.put(b + 0x70, &[1]);
        let mut p = VisibleProbe::default();
        assert_eq!(p.read(&m, &o, a), Some(false));
        assert!(p.calibrate(&m, &o, &[a, b], &[c], &[]));
        assert_eq!(p.read(&m, &o, a), Some(true));
        assert_eq!(p.read(&m, &o, c), Some(false));
        // anchors in the other state than expected: nothing fits, nothing changes
        let mut q = VisibleProbe::default();
        q.calibrate(&m, &o, &[c], &[a], &[]);
        assert_eq!(q.off, None);
        p.last_check = None; // as if 500 ms passed
        assert!(p.calibrate(&m, &o, &[c], &[a], &[])); // disagrees: rescans, finds nothing, keeps 0x70
        assert_eq!(p.off, Some(0x70));
    }

    #[test]
    fn visible_probe_drops_a_wrong_offset_it_found_earlier() {
        let (mut m, o) = (FakeMemory::default(), offs());
        let (a, b, c) = (
            m.alloc(VISIBLE_WINDOW),
            m.alloc(VISIBLE_WINDOW),
            m.alloc(VISIBLE_WINDOW),
        );
        for g in [a, b, c] {
            m.put(g, &[0; VISIBLE_WINDOW]);
        }
        m.put(a + 0x70, &[1]);
        m.put(b + 0x70, &[1]);
        let mut p = VisibleProbe {
            off: Some(0x90), // found while the game was loading, wrong now
            last_check: None,
        };
        assert!(p.calibrate(&m, &o, &[a, b], &[c], &[]));
        assert_eq!(p.off, Some(0x70));
        assert!(!p.calibrate(&m, &o, &[a, b], &[c], &[])); // checked again only after 500 ms
    }

    #[test]
    fn read_gui_fields() {
        let (mut m, o) = (FakeMemory::default(), offs());
        let label = m.instance("TextLabel", "L", &[]);
        m.put(label + 0x58, &[1]);
        assert_eq!(gui_visible(&m, &o, label), Some(true));
        m.put(label + 0x58, &[2]); // only 1 counts as visible, as the games read it
        assert_eq!(gui_visible(&m, &o, label), Some(false));

        let s = m.str("+45xp"); // copy the 32-byte string header into the label's Text field
        for i in 0..32 {
            let b = m.bytes.get(&(s + i)).copied().unwrap_or(0);
            m.bytes.insert(label + 0x60 + i, b);
        }
        assert_eq!(gui_text(&m, &o, label).as_deref(), Some("+45xp"));

        let size = UDim2 {
            x_scale: 0.5,
            x_offset: 10,
            y_scale: 0.25,
            y_offset: -4,
        };
        m.put_val(label + 0xB0, &size);
        assert_eq!(gui_size(&m, &o, label), Some(size));
    }

    #[test]
    fn read_value_objects() {
        let (mut m, o) = (FakeMemory::default(), offs());
        let (num, obj) = (m.instance("NumberValue", "N", &[]), m.instance("ObjectValue", "O", &[]));
        m.put_val(num + 0x80, &2.5f64);
        assert_eq!(number_value(&m, &o, num), Some(2.5));
        m.put_val(num + 0x80, &-3i64);
        assert_eq!(int_value(&m, &o, num), Some(-3));
        m.put_ptr(obj + 0x80, num);
        assert_eq!(object_value(&m, &o, obj), num);
        let sv = m.instance("StringValue", "S", &[]);
        assert!(string_value(&m, &o, sv).is_none());
    }

    #[test]
    fn read_tree_walks() {
        let (mut m, o) = (FakeMemory::default(), offs());
        let net = m.instance("Part", "Net", &[]);
        let map = m.instance("Folder", "Map", &[net]);
        let ws = m.instance("Workspace", "Workspace", &[map]);
        let dm = m.instance("DataModel", "Game", &[ws]);
        parent(&mut m, net, map);
        parent(&mut m, map, ws);
        parent(&mut m, ws, dm);
        assert_eq!(parent_of(&m, &o, net), map);
        assert_eq!(child_count(&m, &o, ws), 1);
        assert_eq!(child_count(&m, &o, net), 0);
    }

    // How many reads (ReadProcessMemory calls against Roblox) the tree walks cost; a rise here is a slowdown.
    struct Tree {
        m: FakeMemory,
        o: Offsets,
        kids: Vec<usize>,
        folder: usize,
    }

    impl Tree {
        fn new() -> Self {
            let mut m = FakeMemory::default();
            let kids: Vec<usize> = (0..50).map(|i| m.instance("Part", &format!("Part{i}"), &[])).collect();
            let folder = m.instance("Folder", "Folder", &kids);
            Self {
                m,
                o: fake_instance_offsets(),
                kids,
                folder,
            }
        }
        fn cost(&self, f: impl FnOnce()) -> usize {
            self.m.reads.set(0);
            f();
            self.m.reads.get()
        }
    }

    #[test]
    fn cost_strings() {
        let mut t = Tree::new();
        let (inl, heap) = (t.m.str("Bump"), t.m.str("a name longer than fifteen"));
        let (m, o) = (&t.m, &t.o);
        assert_eq!(
            t.cost(|| assert_eq!(read_string(m, o, inl).as_deref(), Some("Bump"))),
            1
        );
        assert_eq!(
            t.cost(|| assert_eq!(read_string(m, o, heap).as_deref(), Some("a name longer than fifteen"))),
            2
        );
        assert_eq!(
            t.cost(|| {
                let _ = instance_name(m, o, t.kids[0]);
            }),
            2
        );
        assert_eq!(
            t.cost(|| {
                let _ = instance_class(m, o, t.kids[0]);
            }),
            3
        );
    }

    #[test]
    fn cost_children() {
        let t = Tree::new();
        let (m, o) = (&t.m, &t.o);
        assert_eq!(t.cost(|| assert_eq!(children(m, o, t.folder), t.kids)), 3);
        assert_eq!(t.cost(|| assert_eq!(child_count(m, o, t.folder), 50)), 2);
        assert_eq!(
            t.cost(|| assert_eq!(child_by_name(m, o, t.folder, "Part49"), t.kids[49])),
            3 + 50 * 2
        );
        assert_eq!(
            t.cost(|| {
                let _ = child_by_class(m, o, t.folder, "Model");
            }),
            3 + 50 * 3
        );
    }

    #[test]
    fn cost_class_names_cached_per_descriptor() {
        let mut m = FakeMemory {
            cache: Some(Default::default()),
            ..Default::default()
        };
        let o = fake_instance_offsets();
        let (part, model) = (m.instance("Part", "P", &[]), m.instance("Model", "M", &[]));
        assert_eq!(instance_class(&m, &o, part).as_deref(), Some("Part"));
        m.reads.set(0);
        assert_eq!(instance_class(&m, &o, part).as_deref(), Some("Part"));
        assert_eq!(m.reads.get(), 1); // the descriptor pointer only
        assert_eq!(instance_class(&m, &o, model).as_deref(), Some("Model")); // another descriptor: read
    }

    #[test]
    fn cost_cached_path() {
        let mut m = FakeMemory::default();
        let o = fake_instance_offsets();
        let mut kids: Vec<usize> = (0..20).map(|i| m.instance("Part", &format!("J{i}"), &[])).collect();
        let bar = m.instance("Frame", "bar", &[]);
        let power = m.instance("Frame", "power", &[bar]);
        kids.push(power);
        let root = m.instance("Part", "Root", &kids);

        let mut path = CachedPath::new(&["power", "bar"]);
        assert_eq!(path.get(&m, &o, root), bar);
        m.reads.set(0);
        assert_eq!(path.get(&m, &o, root), bar);
        assert_eq!(m.reads.get(), 2 * 3); // one children list per level, no names
        let list = m.get_ptr(power + 0x30);
        m.put_ptr(power + 0x30, 0); // bar destroyed: power's list no longer holds it
        assert_eq!(path.get(&m, &o, root), 0);
        m.reads.set(0);
        assert_eq!(path.get(&m, &o, root), 0);
        assert_eq!(m.reads.get(), 3 + 1); // root's list holds power; power's list is still unreadable: no names read
        let mut absent = CachedPath::new(&["missing"]);
        assert_eq!(absent.get(&m, &o, root), 0);
        m.reads.set(0);
        assert_eq!(absent.get(&m, &o, root), 0);
        assert_eq!(m.reads.get(), 3); // root's children unchanged since the miss: not read by name again
        m.put_ptr(power + 0x30, list);
        assert_eq!(path.get(&m, &o, root), bar);
        assert_eq!(path.get(&m, &o, 0), 0);
        assert_eq!(path.get(&m, &o, root), bar); // a new root starts over
        assert_eq!(CachedPath::default().get(&m, &o, root), root);
    }
}
