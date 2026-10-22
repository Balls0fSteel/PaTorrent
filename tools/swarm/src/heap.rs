//! A counting global allocator: the live heap bytes and blocks, so `load` can report what the
//! engine keeps per torrent apart from what the working set adds (the heap the allocator keeps
//! after the restore's transient peak, and its overhead per block). It counts only once
//! `enable` was called at startup, so the throughput benchmarks run without the shared counters.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};

pub struct Counting;

static ON: AtomicBool = AtomicBool::new(false);
static BYTES: AtomicIsize = AtomicIsize::new(0);
static BLOCKS: AtomicIsize = AtomicIsize::new(0);

fn count(bytes: isize, blocks: isize) {
    if ON.load(Ordering::Relaxed) {
        BYTES.fetch_add(bytes, Ordering::Relaxed);
        BLOCKS.fetch_add(blocks, Ordering::Relaxed);
    }
}

// SAFETY: forwards every call to the system allocator unchanged; it only counts.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() {
            count(layout.size() as isize, 1);
        }
        p
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { System.alloc_zeroed(layout) };
        if !p.is_null() {
            count(layout.size() as isize, 1);
        }
        p
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) };
        count(-(layout.size() as isize), -1);
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let p = unsafe { System.realloc(ptr, layout, new_size) };
        if !p.is_null() {
            count(new_size as isize - layout.size() as isize, 0);
        }
        p
    }
}

/// Starts counting; call it first thing, before the blocks it should see are allocated (a
/// block allocated before and freed after would count negative).
pub fn enable() {
    ON.store(true, Ordering::Relaxed);
}

/// Live heap bytes and blocks counted since `enable` (requested sizes, without the allocator's
/// own overhead).
pub fn live() -> (u64, u64) {
    (BYTES.load(Ordering::Relaxed).max(0) as u64, BLOCKS.load(Ordering::Relaxed).max(0) as u64)
}
