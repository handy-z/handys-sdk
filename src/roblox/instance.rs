//! Walks the instance tree. Every read() is a ReadProcessMemory round trip into Roblox, so these read whole
//! structs at once where the layout allows, and fall back to one field at a time when that read fails.
use super::memory::Memory;
use super::offsets::Offsets;
use super::process::RobloxProcess;
use super::read::ptr_at;

const MAX_STRING: u64 = 4096;
const MAX_CHILDREN: usize = 4096;

fn u64_at(b: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(b[at..at + 8].try_into().unwrap())
}

/// The children vector's begin and end; None when unreadable.
fn child_bounds(m: &dyn Memory, o: &Offsets, inst: usize) -> Option<(usize, usize)> {
    let vec = ptr_at(m, o.at(inst, "Instance.ChildrenStart"));
    if vec == 0 {
        return None;
    }
    let end_at = o["Instance.ChildrenEnd"];
    let mut b = [0u8; 64];
    let (begin, end) = if (8..=56).contains(&end_at) && m.read(vec, &mut b[..end_at as usize + 8]) {
        (u64_at(&b, 0) as usize, u64_at(&b, end_at as usize) as usize)
    } else {
        (ptr_at(m, vec), ptr_at(m, vec.wrapping_add(end_at as usize)))
    };
    (begin != 0 && end >= begin).then_some((begin, end))
}

/// An MSVC std::string of at most 4096 bytes: a 16-byte inline buffer (or the heap pointer) at 0, size and
/// capacity at Misc.StringLength.
pub fn read_string(m: &dyn Memory, o: &Offsets, s: usize) -> Option<String> {
    let len_at = o["Misc.StringLength"];
    let mut head = [0u8; 64];
    let whole = (16..=48).contains(&len_at) && m.read(s, &mut head[..len_at as usize + 16]);
    let len_at = len_at as usize;
    let (size, cap) = if whole {
        (u64_at(&head, len_at), u64_at(&head, len_at + 8))
    } else {
        (
            m.get::<u64>(s.wrapping_add(len_at))?,
            m.get::<u64>(s.wrapping_add(len_at + 8))?,
        )
    };
    if size > MAX_STRING || (if cap == 15 { size > 15 } else { size > cap }) {
        return None;
    }
    let size = size as usize;
    if cap == 15 && whole {
        return Some(String::from_utf8_lossy(&head[..size]).into_owned());
    }
    let mut data = s;
    if cap != 15 {
        data = if whole { u64_at(&head, 0) as usize } else { ptr_at(m, s) };
        if data == 0 {
            return None;
        }
    }
    let mut out = vec![0u8; size];
    if size > 0 && !m.read(data, &mut out) {
        return None;
    }
    Some(String::from_utf8_lossy_owned(out))
}

pub fn instance_name(m: &dyn Memory, o: &Offsets, inst: usize) -> Option<String> {
    let s = ptr_at(m, o.at(inst, "Instance.NameContainer"));
    if s == 0 { None } else { read_string(m, o, s) }
}

pub fn instance_class(m: &dyn Memory, o: &Offsets, inst: usize) -> Option<String> {
    let d = ptr_at(m, o.at(inst, "Instance.ClassDescriptor"));
    if d == 0 {
        return None;
    }
    let cache = m.class_names();
    if let Some(name) = cache.and_then(|c| c.find(d)) {
        return Some(name);
    }
    let s = ptr_at(m, o.at(d, "Instance.ClassName"));
    if s == 0 {
        return None;
    }
    let name = read_string(m, o, s)?;
    if let Some(c) = cache {
        c.add(d, &name);
    }
    Some(name)
}

pub fn children(m: &dyn Memory, o: &Offsets, inst: usize) -> Vec<usize> {
    let Some((begin, end)) = child_bounds(m, o, inst) else {
        return Vec::new();
    };
    let n = (end - begin) / 16;
    if n > MAX_CHILDREN {
        return Vec::new();
    }
    // vector<shared_ptr<Instance>>: 16-byte entries, the Instance pointer first.
    let mut entries = vec![0usize; 2 * n];
    if n > 0 && m.read_slice(begin, &mut entries) {
        entries.iter().step_by(2).copied().filter(|c| *c != 0).collect()
    } else {
        (0..n).map(|i| ptr_at(m, begin + 16 * i)).filter(|c| *c != 0).collect()
    }
}

/// Number of children from the vector's bounds (two reads); 0 when unreadable.
pub fn child_count(m: &dyn Memory, o: &Offsets, inst: usize) -> usize {
    if inst == 0 {
        return 0;
    }
    child_bounds(m, o, inst).map_or(0, |(begin, end)| (end - begin) / 16)
}

/// 0 if none.
pub fn child_by_name(m: &dyn Memory, o: &Offsets, inst: usize, name: &str) -> usize {
    children(m, o, inst)
        .into_iter()
        .find(|c| instance_name(m, o, *c).as_deref() == Some(name))
        .unwrap_or(0)
}

/// 0 if none.
pub fn child_by_class(m: &dyn Memory, o: &Offsets, inst: usize, class: &str) -> usize {
    children(m, o, inst)
        .into_iter()
        .find(|c| instance_class(m, o, *c).as_deref() == Some(class))
        .unwrap_or(0)
}

#[derive(Debug, Default, PartialEq)]
pub struct PathResult {
    pub inst: usize,
    /// The first name that was not found; empty on success.
    pub failed_at: String,
}

pub fn resolve_path(m: &dyn Memory, o: &Offsets, root: usize, names: &[&str]) -> PathResult {
    let mut inst = root;
    for n in names {
        inst = child_by_name(m, o, inst, n);
        if inst == 0 {
            return PathResult {
                inst: 0,
                failed_at: (*n).to_owned(),
            };
        }
    }
    PathResult {
        inst,
        failed_at: String::new(),
    }
}

/// 0 on failure.
pub fn data_model(p: &RobloxProcess, o: &Offsets) -> usize {
    let fake = ptr_at(p, o.at(p.base(), "FakeDataModel.Pointer"));
    if fake == 0 {
        0
    } else {
        ptr_at(p, o.at(fake, "FakeDataModel.RealDataModel"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{FakeMemory, fake_instance_offsets};

    // Like FakeMemory::instance but without the control-block halves of the children entries, so the bulk read of
    // the entry array fails and the one-pointer-at-a-time fallback runs.
    fn node(m: &mut FakeMemory, class: &str, name: &str, kids: &[usize]) -> usize {
        let inst = m.instance(class, name, &[]);
        if !kids.is_empty() {
            let (vec, arr) = (m.alloc(0x20), m.alloc(16 * kids.len()));
            m.put_ptr(vec, arr);
            m.put_ptr(vec + 8, arr + 16 * kids.len());
            for (i, k) in kids.iter().enumerate() {
                m.put_ptr(arr + 16 * i, *k);
            }
            m.put_ptr(inst + 0x30, vec);
        }
        inst
    }

    // Raw string header: size@16, cap@24; heap data (if any) mapped for `heap_bytes` bytes only.
    fn raw_str(m: &mut FakeMemory, size: u64, cap: u64, heap_bytes: usize) -> usize {
        let a = m.alloc(32);
        m.put_val(a + 16, &size);
        m.put_val(a + 24, &cap);
        if cap == 15 {
            m.put(a, &[b'a'; 16]);
        } else {
            let h = m.alloc(heap_bytes + 1);
            m.put(h, &vec![b'a'; heap_bytes]);
            m.put_ptr(a, h);
        }
        a
    }

    #[test]
    fn instance_string_inline_and_heap() {
        let (mut m, o) = (FakeMemory::default(), fake_instance_offsets());
        let (a, b) = (m.str("Bump"), m.str("SHIELD BREAKER 100%"));
        assert_eq!(read_string(&m, &o, a).as_deref(), Some("Bump"));
        assert_eq!(read_string(&m, &o, b).as_deref(), Some("SHIELD BREAKER 100%"));
        assert_eq!(read_string(&m, &o, 0x999999), None);
    }

    #[test]
    fn instance_string_guards() {
        let (mut m, o) = (FakeMemory::default(), fake_instance_offsets());
        let cases = [
            (5000, 8000, 5000, false),
            (16, 15, 0, false),
            (40, 31, 40, false),
            (40, 63, 20, false),
        ];
        for (size, cap, heap, ok) in cases.into_iter().chain([(20, 31, 20, true)]) {
            let s = raw_str(&mut m, size, cap, heap);
            assert_eq!(read_string(&m, &o, s).is_some(), ok, "size {size} cap {cap}");
        }
    }

    #[test]
    fn instance_null_children_is_empty() {
        let (mut m, o) = (FakeMemory::default(), fake_instance_offsets());
        let i = node(&mut m, "Folder", "X", &[]);
        assert!(children(&m, &o, i).is_empty());
        assert_eq!(instance_name(&m, &o, i).as_deref(), Some("X"));
        assert_eq!(instance_class(&m, &o, i).as_deref(), Some("Folder"));
    }

    #[test]
    fn instance_resolve_reports_failed_segment() {
        let (mut m, o) = (FakeMemory::default(), fake_instance_offsets());
        let game = node(&mut m, "Frame", "Game", &[]);
        let iface = node(&mut m, "ScreenGui", "Interface", &[game]);
        let pg = node(&mut m, "PlayerGui", "PlayerGui", &[iface]);
        let bad = resolve_path(&m, &o, pg, &["Interface", "Game", "InGameActionsBar"]);
        assert_eq!(
            bad,
            PathResult {
                inst: 0,
                failed_at: "InGameActionsBar".into()
            }
        );
        assert_eq!(resolve_path(&m, &o, pg, &["Interface", "Game"]).inst, game);
        assert_eq!(child_by_class(&m, &o, pg, "ScreenGui"), iface);
    }

    #[test]
    fn instance_children_guards() {
        let (mut m, o) = (FakeMemory::default(), fake_instance_offsets());
        let i = node(&mut m, "Folder", "X", &[]);
        let (vec, arr) = (m.alloc(0x20), m.alloc(64));
        m.put_ptr(i + 0x30, vec);
        m.put_ptr(vec, arr + 32);
        m.put_ptr(vec + 8, arr); // end < begin
        assert!(children(&m, &o, i).is_empty());
        m.put_ptr(vec, arr);
        m.put_ptr(vec + 8, arr + 16 * 100_001); // count > cap
        assert!(children(&m, &o, i).is_empty());
    }
}
