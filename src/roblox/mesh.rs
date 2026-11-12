//! Reads the meshes Roblox has already downloaded and decoded, straight from its content-provider caches, so an app
//! can draw a part's real shape without fetching assets itself. Every key here is optional: apps check
//! `mesh_keys_present` and leave the keys out of their required list, so a dump without them only loses meshes.
use std::collections::{HashMap, HashSet};

use super::instance::{child_by_class, instance_class, read_string};
use super::memory::{Memory, Pod};
use super::offsets::Offsets;
use super::read::{primitive_of, ptr_at};

/// Offset keys this module reads. Optional: an app must NOT add them to its required list; check mesh_keys_present
/// first.
pub const MESH_KEYS: [&str; 17] = [
    "BasePart.Primitive",
    "Primitive.Rotation",
    "Primitive.Position",
    "Primitive.Size",
    "MeshPart.MeshId",
    "UnionOperation.AssetId",
    "MeshContentProvider.LRUHolder",
    "LRUHolder.MemEnforcedLRUCache",
    "MemEnforcedLRUCache.Head",
    "LRUNode.Next",
    "LRUNode.AssetID",
    "LRUNode.CachedItem",
    "CachedItem.FileMeshData",
    "FileMeshData.Vertices",
    "FileMeshData.VerticesEnd",
    "FileMeshData.Faces",
    "FileMeshData.FacesEnd",
];

pub fn mesh_keys_present(o: &Offsets) -> bool {
    o.has_all(&MESH_KEYS)
}

// The parts seen live have 72..605 vertices; anything past this is garbage from a stale or misread pointer.
const MAX_ELEMS: usize = 200_000;
// The LRU list may be circular or point into freed memory, so the walk is bounded.
const MAX_NODES: usize = 10_000;
// FileMeshData vertices: position f32x3 first, then normal, uv and the rest.
const VERTEX_STRIDE: usize = 40;

// shortcut: not in the offset dump, found by memory search on version-4ea8e85481e34f46; search again when unions stop showing.
const UNION_ITEM_DATA: usize = 0x28;
const UNION_DATA_MESH: usize = 0xd8;
const UNION_POSITIONS: usize = 0x40;
const UNION_UVS: usize = 0xa0;
const UNION_INDICES: usize = 0xd0;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Mesh {
    pub positions: Vec<[f32; 3]>,
    pub tris: Vec<[u32; 3]>,
}

/// The trailing number of an asset URL ("rbxassetid://1", "https://...?id=1").
fn trailing_id(s: &str) -> Option<u64> {
    let digits = s.len() - s.trim_end_matches(|c: char| c.is_ascii_digit()).len();
    s[s.len() - digits..].parse().ok()
}

/// A part's mesh asset id and whether it is a union (PartOperation/UnionOperation) rather than a MeshPart; None for
/// other classes or no id.
pub fn part_mesh_id(m: &dyn Memory, o: &Offsets, part: usize) -> Option<(u64, bool)> {
    if part == 0 || !mesh_keys_present(o) {
        return None;
    }
    let (key, union) = match instance_class(m, o, part)?.as_str() {
        "MeshPart" => ("MeshPart.MeshId", false),
        "PartOperation" | "UnionOperation" => ("UnionOperation.AssetId", true),
        _ => return None,
    };
    Some((trailing_id(&read_string(m, o, o.at(part, key))?)?, union))
}

/// rotation (row-major 3x3), position, size of a BasePart
pub fn part_frame(m: &dyn Memory, o: &Offsets, part: usize) -> Option<([f32; 9], [f32; 3], [f32; 3])> {
    if part == 0 || !mesh_keys_present(o) {
        return None;
    }
    let prim = primitive_of(m, o, part);
    if prim == 0 {
        return None;
    }
    Some((
        m.get(o.at(prim, "Primitive.Rotation"))?,
        m.get(o.at(prim, "Primitive.Position"))?,
        m.get(o.at(prim, "Primitive.Size"))?,
    ))
}

/// Element count of a [begin, end) vector of `size`-byte items; None when it is not whole, empty or too big.
fn count(begin: usize, end: usize, size: usize) -> Option<usize> {
    let bytes = end.checked_sub(begin)?;
    let n = bytes / size;
    (begin != 0 && bytes % size == 0 && (1..=MAX_ELEMS).contains(&n)).then_some(n)
}

fn read_vec<T: Pod + Default>(m: &dyn Memory, begin: usize, n: usize) -> Option<Vec<T>> {
    let mut v = vec![T::default(); n];
    m.read_slice(begin, &mut v).then_some(v)
}

fn checked(positions: Vec<[f32; 3]>, tris: Vec<[u32; 3]>) -> Option<Mesh> {
    let n = positions.len() as u32;
    tris.iter()
        .flatten()
        .all(|i| *i < n)
        .then_some(Mesh { positions, tris })
}

fn mesh_part_mesh(m: &dyn Memory, o: &Offsets, item: usize) -> Option<Mesh> {
    let fmd = ptr_at(m, o.at(item, "CachedItem.FileMeshData"));
    if fmd == 0 {
        return None;
    }
    let field = |k| ptr_at(m, o.at(fmd, k));
    let (vb, ve) = (field("FileMeshData.Vertices"), field("FileMeshData.VerticesEnd"));
    let (fb, fe) = (field("FileMeshData.Faces"), field("FileMeshData.FacesEnd"));
    let verts: Vec<[f32; VERTEX_STRIDE / 4]> = read_vec(m, vb, count(vb, ve, VERTEX_STRIDE)?)?;
    let tris = read_vec(m, fb, count(fb, fe, 12)?)?;
    checked(verts.iter().map(|v| [v[0], v[1], v[2]]).collect(), tris)
}

fn union_mesh(m: &dyn Memory, item: usize) -> Option<Mesh> {
    let mesh = ptr_at(m, ptr_at(m, item + UNION_ITEM_DATA).wrapping_add(UNION_DATA_MESH));
    if mesh == 0 {
        return None;
    }
    // One read covers the three vectors' begin/end pointers.
    let mut h = [0usize; (UNION_INDICES + 16 - UNION_POSITIONS) / 8];
    if !m.read_slice(mesh + UNION_POSITIONS, &mut h) {
        return None;
    }
    let vec_at = |off: usize| (h[(off - UNION_POSITIONS) / 8], h[(off - UNION_POSITIONS) / 8 + 1]);
    let ((pb, pe), (ub, ue), (ib, ie)) = (vec_at(UNION_POSITIONS), vec_at(UNION_UVS), vec_at(UNION_INDICES));
    let n = count(pb, pe, 12)?;
    // The uv vector running parallel to positions is what tells this layout apart from some other object.
    if count(ub, ue, 8)? != n {
        return None;
    }
    let positions = read_vec(m, pb, n)?;
    let tris = read_vec(m, ib, count(ib, ie, 12)?)?;
    checked(positions, tris)
}

/// The decoded meshes for `wanted` ids (id, is_union) from the game's caches: walks each needed provider's LRU once
/// and reads only the wanted entries. Missing or invalid ones are left out.
pub fn cached_meshes(m: &dyn Memory, o: &Offsets, dm: usize, wanted: &[(u64, bool)]) -> HashMap<u64, Mesh> {
    let mut out = HashMap::new();
    if dm == 0 || !mesh_keys_present(o) {
        return out;
    }
    for (class, union) in [("MeshContentProvider", false), ("SolidModelContentProvider", true)] {
        let mut want: HashSet<u64> = wanted.iter().filter(|w| w.1 == union).map(|w| w.0).collect();
        if want.is_empty() {
            continue;
        }
        let prov = child_by_class(m, o, dm, class);
        if prov == 0 {
            continue;
        }
        let holder = ptr_at(m, o.at(prov, "MeshContentProvider.LRUHolder"));
        let cache = if holder == 0 {
            0
        } else {
            ptr_at(m, o.at(holder, "LRUHolder.MemEnforcedLRUCache"))
        };
        let mut node = if cache == 0 {
            0
        } else {
            ptr_at(m, o.at(cache, "MemEnforcedLRUCache.Head"))
        };
        let mut seen = HashSet::new();
        while node != 0 && !want.is_empty() && seen.len() < MAX_NODES && seen.insert(node) {
            let id = read_string(m, o, o.at(node, "LRUNode.AssetID")).and_then(|s| trailing_id(&s));
            if let Some(id) = id.filter(|id| want.contains(id)) {
                // Removed even when invalid: a second node with the same id is the same asset.
                want.remove(&id);
                let item = ptr_at(m, o.at(node, "LRUNode.CachedItem"));
                let mesh = match (item, union) {
                    (0, _) => None,
                    (_, false) => mesh_part_mesh(m, o, item),
                    (_, true) => union_mesh(m, item),
                };
                if let Some(mesh) = mesh {
                    out.insert(id, mesh);
                }
            }
            node = ptr_at(m, o.at(node, "LRUNode.Next"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{FakeMemory, fake_instance_offsets};

    // Part and provider fields go in the free space past 0x40 of FakeMemory::instance.
    fn offs() -> Offsets {
        let mut o = fake_instance_offsets();
        o.values.extend(
            [
                ("BasePart.Primitive", 0x50),
                ("Primitive.Rotation", 0x0),
                ("Primitive.Position", 0x24),
                ("Primitive.Size", 0x30),
                ("MeshPart.MeshId", 0x60),
                ("UnionOperation.AssetId", 0x60),
                ("MeshContentProvider.LRUHolder", 0x48),
                ("LRUHolder.MemEnforcedLRUCache", 0x20),
                ("MemEnforcedLRUCache.Head", 0x8),
                ("LRUNode.Next", 0x0),
                ("LRUNode.AssetID", 0x10),
                ("LRUNode.CachedItem", 0x38),
                ("CachedItem.FileMeshData", 0x28),
                ("FileMeshData.Vertices", 0x0),
                ("FileMeshData.VerticesEnd", 0x8),
                ("FileMeshData.Faces", 0x30),
                ("FileMeshData.FacesEnd", 0x38),
            ]
            .map(|(k, v)| (k.to_owned(), v)),
        );
        o
    }

    // Copies an MSVC string built by FakeMemory::str to `at` (the heap pointer, if any, stays shared).
    fn put_str(m: &mut FakeMemory, at: usize, s: &str) {
        let a = m.str(s);
        let b: [u8; 32] = (&*m as &dyn Memory).get(a).unwrap();
        m.put(at, &b);
    }

    fn put_vec<T: Pod>(m: &mut FakeMemory, at: usize, items: &[T]) {
        let a = m.alloc(size_of_val(items));
        for (i, v) in items.iter().enumerate() {
            m.put_val(a + i * size_of::<T>(), v);
        }
        m.put_ptr(at, a);
        m.put_ptr(at + 8, a + size_of_val(items));
    }

    fn lru_node(m: &mut FakeMemory, url: &str, item: usize, next: usize) -> usize {
        let n = m.alloc(0x40);
        m.put_ptr(n, next);
        put_str(m, n + 0x10, url);
        m.put_ptr(n + 0x38, item);
        n
    }

    // A provider instance whose LRU list starts at `head`.
    fn provider(m: &mut FakeMemory, class: &str, head: usize) -> usize {
        let (p, holder, cache) = (m.instance(class, class, &[]), m.alloc(0x30), m.alloc(0x10));
        m.put_ptr(p + 0x48, holder);
        m.put_ptr(holder + 0x20, cache);
        m.put_ptr(cache + 0x8, head);
        p
    }

    const TRI: [[f32; 3]; 3] = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];

    fn mesh_part_item(m: &mut FakeMemory) -> usize {
        let (item, fmd) = (m.alloc(0x30), m.alloc(0x40));
        m.put_ptr(item + 0x28, fmd);
        let verts: Vec<[f32; 10]> = TRI
            .iter()
            .map(|p| [p[0], p[1], p[2], 9., 9., 9., 9., 9., 9., 9.])
            .collect();
        put_vec(m, fmd, &verts);
        put_vec(m, fmd + 0x30, &[[0u32, 1, 2]]);
        item
    }

    fn union_item(m: &mut FakeMemory, indices: &[u32]) -> usize {
        let (item, data, mesh) = (m.alloc(0x30), m.alloc(0xe0), m.alloc(0xe0));
        m.put_ptr(item + UNION_ITEM_DATA, data);
        m.put_ptr(data + UNION_DATA_MESH, mesh);
        m.put(mesh + UNION_POSITIONS, &[0; UNION_INDICES + 16 - UNION_POSITIONS]);
        put_vec(m, mesh + UNION_POSITIONS, &TRI);
        put_vec(m, mesh + UNION_UVS, &[[0f32; 2]; 3]);
        put_vec(m, mesh + UNION_INDICES, indices);
        item
    }

    const URL: &str = "https://assetdelivery.roblox.com/v1/asset?id=";

    // A DataModel with a MeshPart mesh (id 111, behind an unrelated node 999) and a union mesh (id 222).
    fn world(m: &mut FakeMemory, union_indices: &[u32]) -> usize {
        let item = mesh_part_item(m);
        let wanted = lru_node(m, &format!("{URL}111"), item, 0);
        let other_item = mesh_part_item(m);
        let other = lru_node(m, &format!("{URL}999"), other_item, wanted);
        let mp = provider(m, "MeshContentProvider", other);
        let uitem = union_item(m, union_indices);
        let un = lru_node(m, &format!("{URL}222"), uitem, 0);
        let sp = provider(m, "SolidModelContentProvider", un);
        let ws = m.instance("Workspace", "Workspace", &[]);
        m.instance("DataModel", "Game", &[ws, mp, sp])
    }

    fn tri_mesh() -> Mesh {
        Mesh {
            positions: TRI.to_vec(),
            tris: vec![[0, 1, 2]],
        }
    }

    #[test]
    fn mesh_cached_finds_both_kinds_by_id() {
        let (mut m, o) = (FakeMemory::default(), offs());
        let dm = world(&mut m, &[0, 1, 2]);
        let got = cached_meshes(&m, &o, dm, &[(111, false), (222, true), (333, false)]);
        assert_eq!(got.len(), 2);
        assert_eq!(got[&111], tri_mesh());
        assert_eq!(got[&222], tri_mesh());
        // The union id is looked up only in the union cache.
        assert!(cached_meshes(&m, &o, dm, &[(222, false)]).is_empty());
    }

    #[test]
    fn mesh_cached_rejects_bad_union() {
        let (mut m, o) = (FakeMemory::default(), offs());
        let dm = world(&mut m, &[0, 1, 3]);
        assert!(cached_meshes(&m, &o, dm, &[(222, true)]).is_empty());
        let dm = world(&mut m, &[0, 1]); // not whole triangles
        assert!(cached_meshes(&m, &o, dm, &[(222, true)]).is_empty());
    }

    #[test]
    fn mesh_cached_stops_on_circular_list() {
        let (mut m, o) = (FakeMemory::default(), offs());
        let item = mesh_part_item(&mut m);
        let a = lru_node(&mut m, &format!("{URL}1"), item, 0);
        let b = lru_node(&mut m, &format!("{URL}2"), item, a);
        m.put_ptr(a, b);
        let p = provider(&mut m, "MeshContentProvider", a);
        let dm = m.instance("DataModel", "Game", &[p]);
        assert!(cached_meshes(&m, &o, dm, &[(3, false)]).is_empty());
        assert_eq!(cached_meshes(&m, &o, dm, &[(2, false)])[&2], tri_mesh());
    }

    #[test]
    fn mesh_read_cost() {
        let (mut m, o) = (FakeMemory::default(), offs());
        let dm = world(&mut m, &[0, 1, 2]);
        let p = child_by_class(&m, &o, dm, "MeshContentProvider");
        m.reads.set(0);
        child_by_class(&m, &o, dm, "MeshContentProvider");
        let find = m.reads.get();
        let node = ptr_at(&m, ptr_at(&m, ptr_at(&m, p + 0x48) + 0x20) + 0x8);
        m.reads.set(0);
        read_string(&m, &o, node + 0x10);
        // A heap string: its header, then its bytes; plus Next.
        let per_node = m.reads.get() + 1;
        assert_eq!(per_node, 3);
        m.reads.set(0);
        cached_meshes(&m, &o, dm, &[(111, false)]);
        // provider lookup, holder/cache/head, two nodes, then item, fmd, four vector pointers, two slices.
        assert_eq!(m.reads.get(), find + 3 + 2 * per_node + 8);
    }

    #[test]
    fn mesh_part_id_and_frame() {
        let (mut m, o) = (FakeMemory::default(), offs());
        let mp = m.instance("MeshPart", "M", &[]);
        put_str(&mut m, mp + 0x60, "rbxassetid://135963157793439");
        let un = m.instance("UnionOperation", "U", &[]);
        put_str(&mut m, un + 0x60, "https://www.roblox.com//asset/?id=81735568210910");
        let po = m.instance("PartOperation", "P", &[]);
        put_str(&mut m, po + 0x60, "rbxassetid://");
        let part = m.instance("Part", "B", &[]);
        assert_eq!(part_mesh_id(&m, &o, mp), Some((135963157793439, false)));
        assert_eq!(part_mesh_id(&m, &o, un), Some((81735568210910, true)));
        assert_eq!(part_mesh_id(&m, &o, po), None);
        assert_eq!(part_mesh_id(&m, &o, part), None);
        assert_eq!(part_mesh_id(&m, &o, 0), None);

        let prim = m.alloc(0x40);
        m.put_ptr(mp + 0x50, prim);
        let rot = [1f32, 2., 3., 4., 5., 6., 7., 8., 9.];
        m.put_val(prim, &rot);
        m.put_val(prim + 0x24, &[10f32, 11., 12.]);
        m.put_val(prim + 0x30, &[2f32, 3., 4.]);
        assert_eq!(part_frame(&m, &o, mp), Some((rot, [10., 11., 12.], [2., 3., 4.])));
        assert_eq!(part_frame(&m, &o, part), None);
    }

    #[test]
    fn mesh_missing_keys_give_nothing() {
        let (mut m, mut o) = (FakeMemory::default(), offs());
        let dm = world(&mut m, &[0, 1, 2]);
        let mp = m.instance("MeshPart", "M", &[]);
        put_str(&mut m, mp + 0x60, "rbxassetid://1");
        o.values.remove("FileMeshData.FacesEnd");
        assert!(!mesh_keys_present(&o));
        assert!(cached_meshes(&m, &o, dm, &[(111, false), (222, true)]).is_empty());
        assert_eq!(part_mesh_id(&m, &o, mp), None);
        assert_eq!(part_frame(&m, &o, mp), None);
    }
}
