//! Test support shared with the games: a byte-map `Memory` that builds MSVC strings and instances.
use std::cell::Cell;
use std::collections::BTreeMap;

use crate::roblox::memory::{ClassNameCache, Memory, Pod};
use crate::roblox::offsets::Offsets;

/// The instance layout `FakeMemory::instance` builds; add a test's own keys to `values`.
pub fn fake_instance_offsets() -> Offsets {
    Offsets::from_pairs(&[
        ("Instance.ClassDescriptor", 0x18),
        ("Instance.ClassName", 0x8),
        ("Instance.NameContainer", 0x20),
        ("Instance.ChildrenStart", 0x30),
        ("Instance.ChildrenEnd", 0x8),
        ("Misc.StringLength", 16),
    ])
}

/// `reads` counts read() calls (each is one ReadProcessMemory against Roblox), so tests can pin how many a walk
/// costs. Set `cache` to get a class-name cache like the real process has.
pub struct FakeMemory {
    pub bytes: BTreeMap<usize, u8>,
    pub next: usize,
    pub reads: Cell<usize>,
    pub cache: Option<ClassNameCache>,
}

impl Default for FakeMemory {
    fn default() -> Self {
        Self {
            bytes: BTreeMap::new(),
            next: 0x10000,
            reads: Cell::new(0),
            cache: None,
        }
    }
}

impl Memory for FakeMemory {
    fn read(&self, addr: usize, out: &mut [u8]) -> bool {
        self.reads.set(self.reads.get() + 1);
        for (i, b) in out.iter_mut().enumerate() {
            match self.bytes.get(&(addr.wrapping_add(i))) {
                Some(v) => *b = *v,
                None => return false,
            }
        }
        true
    }
    fn class_names(&self) -> Option<&ClassNameCache> {
        self.cache.as_ref()
    }
}

impl FakeMemory {
    pub fn put(&mut self, addr: usize, data: &[u8]) {
        for (i, b) in data.iter().enumerate() {
            self.bytes.insert(addr + i, *b);
        }
    }
    pub fn put_val<T: Pod>(&mut self, addr: usize, v: &T) {
        // SAFETY: a Pod value is plain bytes.
        self.put(addr, unsafe {
            std::slice::from_raw_parts((v as *const T).cast::<u8>(), size_of::<T>())
        });
    }
    pub fn put_ptr(&mut self, addr: usize, v: usize) {
        self.put_val(addr, &(v as u64));
    }
    pub fn get_ptr(&self, addr: usize) -> usize {
        (self as &dyn Memory).get::<usize>(addr).unwrap_or(0)
    }
    /// Reserves `n` bytes (unmapped until written) with a gap after them.
    pub fn alloc(&mut self, n: usize) -> usize {
        let a = self.next;
        self.next += ((n + 15) & !15) + 16;
        a
    }
    /// MSVC std::string: 16-byte buffer or heap pointer, size@16, cap@24.
    pub fn str(&mut self, s: &str) -> usize {
        let a = self.alloc(32);
        let (size, cap) = (s.len() as u64, if s.len() <= 15 { 15u64 } else { 31 });
        self.put(a, &[0; 16]); // the whole buffer is mapped, as in a real string
        let mut z = s.as_bytes().to_vec();
        z.push(0);
        if cap == 15 {
            self.put(a, &z);
        } else {
            let h = self.alloc(64);
            self.put(h, &z);
            self.put_ptr(a, h);
        }
        self.put_val(a + 16, &size);
        self.put_val(a + 24, &cap);
        a
    }
    /// 0x100 bytes laid out as `fake_instance_offsets()`; fields past 0x40 are free for a test's own offsets.
    pub fn instance(&mut self, class: &str, name: &str, kids: &[usize]) -> usize {
        let (inst, desc) = (self.alloc(0x100), self.alloc(0x20));
        self.put_ptr(inst + 0x18, desc);
        let (c, n) = (self.str(class), self.str(name));
        self.put_ptr(desc + 0x8, c);
        self.put_ptr(inst + 0x20, n);
        if !kids.is_empty() {
            let (vec, arr) = (self.alloc(0x20), self.alloc(16 * kids.len()));
            self.put_ptr(vec, arr);
            self.put_ptr(vec + 8, arr + 16 * kids.len());
            for (i, k) in kids.iter().enumerate() {
                self.put_ptr(arr + 16 * i, *k);
                self.put_ptr(arr + 16 * i + 8, 0); // the shared_ptr's control block
            }
            self.put_ptr(inst + 0x30, vec);
        }
        inst
    }
}
