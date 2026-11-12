use std::cell::UnsafeCell;
use std::mem::MaybeUninit;
use std::sync::atomic::{AtomicUsize, Ordering};

#[repr(align(64))]
struct Padded(AtomicUsize);

/// Wait-free single-producer/single-consumer ring buffer. N must be a power of two.
/// One thread may call `push` and one other `pop`; that is the caller's contract.
pub struct SpscQueue<T: Copy, const N: usize> {
    buf: UnsafeCell<[MaybeUninit<T>; N]>,
    head: Padded,
    tail: Padded,
}

// SAFETY: the producer only writes slots the consumer cannot see yet (head is published after the write),
// and the consumer only reads slots below head.
unsafe impl<T: Copy + Send, const N: usize> Sync for SpscQueue<T, N> {}

impl<T: Copy, const N: usize> Default for SpscQueue<T, N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Copy, const N: usize> SpscQueue<T, N> {
    pub const fn new() -> Self {
        const { assert!(N.is_power_of_two(), "N must be a power of two") };
        Self {
            buf: UnsafeCell::new([MaybeUninit::uninit(); N]),
            head: Padded(AtomicUsize::new(0)),
            tail: Padded(AtomicUsize::new(0)),
        }
    }

    pub fn push(&self, v: T) -> bool {
        let h = self.head.0.load(Ordering::Relaxed);
        if h.wrapping_sub(self.tail.0.load(Ordering::Acquire)) == N {
            return false;
        }
        unsafe { (*self.buf.get())[h & (N - 1)] = MaybeUninit::new(v) };
        self.head.0.store(h.wrapping_add(1), Ordering::Release);
        true
    }

    pub fn pop(&self) -> Option<T> {
        let t = self.tail.0.load(Ordering::Relaxed);
        if t == self.head.0.load(Ordering::Acquire) {
            return None;
        }
        let v = unsafe { (*self.buf.get())[t & (N - 1)].assume_init() };
        self.tail.0.store(t.wrapping_add(1), Ordering::Release);
        Some(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spsc_fifo_and_full() {
        let q = SpscQueue::<i32, 256>::new();
        for i in 0..256 {
            assert!(q.push(i));
        }
        assert!(!q.push(256));
        for i in 0..256 {
            assert_eq!(q.pop(), Some(i));
        }
        assert_eq!(q.pop(), None);
        assert!(q.push(7));
        assert_eq!(q.pop(), Some(7));
    }
}
