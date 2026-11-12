use std::collections::HashMap;
use std::mem::MaybeUninit;
use std::sync::Mutex;

/// Class names by ClassDescriptor address. A descriptor's name is fixed while the process lives, so
/// `instance_class` reads it once. Thread-safe; drops everything past the cap (garbage descriptors from freed
/// instances).
#[derive(Default)]
pub struct ClassNameCache(Mutex<HashMap<usize, String>>);

impl ClassNameCache {
    const CAP: usize = 4096;

    pub fn find(&self, desc: usize) -> Option<String> {
        self.0.lock().unwrap().get(&desc).cloned()
    }

    pub fn add(&self, desc: usize, name: &str) {
        let mut names = self.0.lock().unwrap();
        if names.len() >= Self::CAP {
            names.clear();
        }
        names.entry(desc).or_insert_with(|| name.to_owned());
    }
}

/// Types that are valid for any bit pattern, so they can be filled straight from another process's memory.
///
/// # Safety
/// The type must have no invalid bit patterns and no padding the reader relies on (plain numbers, and
/// `#[repr(C)]` structs or arrays of them).
pub unsafe trait Pod: Copy + 'static {}
macro_rules! pod {
    ($($t:ty),*) => { $(unsafe impl Pod for $t {})* };
}
pod!(u8, i8, u16, i16, u32, i32, u64, i64, usize, isize, f32, f64);
unsafe impl<T: Pod, const N: usize> Pod for [T; N] {}

/// Somewhere to read bytes from: the Roblox process, or a fake in tests.
pub trait Memory {
    /// Fills all of `out` from `addr`, or returns false. Each call is one round trip into Roblox.
    fn read(&self, addr: usize, out: &mut [u8]) -> bool;
    /// None: no cache, every `instance_class` reads the name.
    fn class_names(&self) -> Option<&ClassNameCache> {
        None
    }
}

impl dyn Memory + '_ {
    pub fn get<T: Pod>(&self, addr: usize) -> Option<T> {
        let mut v = MaybeUninit::<T>::zeroed();
        // SAFETY: the slice covers exactly `v`, and any bytes are a valid T (Pod).
        let bytes = unsafe { std::slice::from_raw_parts_mut(v.as_mut_ptr().cast::<u8>(), size_of::<T>()) };
        self.read(addr, bytes).then(|| unsafe { v.assume_init() })
    }

    /// Fills the whole slice in one read.
    pub fn read_slice<T: Pod>(&self, addr: usize, out: &mut [T]) -> bool {
        // SAFETY: as in `get`, over the slice's own bytes.
        let bytes = unsafe { std::slice::from_raw_parts_mut(out.as_mut_ptr().cast::<u8>(), size_of_val(out)) };
        self.read(addr, bytes)
    }
}
