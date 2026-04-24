use std::alloc::{GlobalAlloc, Layout};
use std::cell::Cell;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::config::CONFIG;

const MAX_GLOBAL_SLABS: usize = 256;
const MAX_SLABS_PER_ARENA: usize = 16;

static SLAB_BASES: [AtomicUsize; MAX_GLOBAL_SLABS] = {
    const ZERO: AtomicUsize = AtomicUsize::new(0);
    [ZERO; MAX_GLOBAL_SLABS]
};
static SLAB_ENDS: [AtomicUsize; MAX_GLOBAL_SLABS] = {
    const ZERO: AtomicUsize = AtomicUsize::new(0);
    [ZERO; MAX_GLOBAL_SLABS]
};
static SLAB_COUNT: AtomicUsize = AtomicUsize::new(0);

fn register_slab(base: *mut u8, capacity: usize) -> usize {
    let idx = SLAB_COUNT.fetch_add(1, Ordering::Relaxed);
    if idx >= MAX_GLOBAL_SLABS {
        std::process::abort();
    }
    SLAB_BASES[idx].store(base as usize, Ordering::Relaxed);
    SLAB_ENDS[idx].store(base as usize + capacity, Ordering::Release);
    idx
}

#[inline]
fn find_global_slab(addr: usize) -> Option<usize> {
    let count = SLAB_COUNT.load(Ordering::Acquire);
    for i in 0..count {
        let base = SLAB_BASES[i].load(Ordering::Relaxed);
        let end = SLAB_ENDS[i].load(Ordering::Relaxed);
        if addr >= base && addr < end {
            return Some(i);
        }
    }
    None
}

fn mmap_slab(size: usize) -> *mut u8 {
    let ptr = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            size,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
            -1,
            0,
        )
    };
    if ptr == libc::MAP_FAILED {
        std::ptr::null_mut()
    } else {
        ptr as *mut u8
    }
}

const NUM_SIZE_CLASSES: usize = 7;
const SIZE_CLASS_SIZES: [usize; NUM_SIZE_CLASSES] = [8, 16, 32, 64, 128, 256, 512];
const MAX_FREE_PER_CLASS: u16 = 512;

#[inline]
fn size_class_index(size: usize) -> Option<usize> {
    // Only return Some for exact power-of-two matches
    match size {
        8 => Some(0),
        16 => Some(1),
        32 => Some(2),
        64 => Some(3),
        128 => Some(4),
        256 => Some(5),
        512 => Some(6),
        _ => None,
    }
}

#[derive(Clone, Copy)]
struct SlabMeta {
    base: *mut u8,
    capacity: usize,
}

#[repr(C)]
struct WorkerArena {
    // Hot: alloc/dealloc path
    base: *mut u8,
    capacity: usize,
    cursor: usize,
    active_base: usize,
    active_end: usize,
    _pad: [usize; 3],

    // Free list state
    free_heads: [*mut u8; NUM_SIZE_CLASSES],
    free_counts: [u16; NUM_SIZE_CLASSES],
    _pad2: u16,

    // Cold: slab management
    active_slab: usize,
    slabs: [SlabMeta; MAX_SLABS_PER_ARENA],
    slab_count: usize,
}

impl WorkerArena {
    fn init(arena: &mut WorkerArena) {
        let slab_base = mmap_slab(CONFIG.arena_slab_size);
        if slab_base.is_null() {
            std::process::abort();
        }
        register_slab(slab_base, CONFIG.arena_slab_size);

        arena.base = slab_base;
        arena.capacity = CONFIG.arena_slab_size;
        arena.cursor = 0;
        arena.active_base = slab_base as usize;
        arena.active_end = slab_base as usize + CONFIG.arena_slab_size;
        arena._pad = [0; 3];
        arena.free_heads = [std::ptr::null_mut(); NUM_SIZE_CLASSES];
        arena.free_counts = [0; NUM_SIZE_CLASSES];
        arena._pad2 = 0;
        arena.active_slab = 0;
        arena.slabs[0] = SlabMeta {
            base: slab_base,
            capacity: CONFIG.arena_slab_size,
        };
        arena.slab_count = 1;
    }

    #[inline]
    fn alloc_bump(&mut self, layout: Layout) -> *mut u8 {
        let align = layout.align();
        let size = layout.size();

        // Try free list for exact power-of-two sizes
        if let Some(cls) = size_class_index(size) {
            let head = self.free_heads[cls];
            if !head.is_null() && (head as usize) & (align - 1) == 0 {
                self.free_heads[cls] = unsafe { *(head as *const *mut u8) };
                self.free_counts[cls] -= 1;
                return head;
            }
        }

        let aligned = (self.cursor + align - 1) & !(align - 1);
        let new_cursor = aligned + size;

        if new_cursor <= self.capacity {
            self.cursor = new_cursor;
            return unsafe { self.base.add(aligned) };
        }

        self.alloc_bump_slow(layout)
    }

    #[cold]
    #[inline(never)]
    fn alloc_bump_slow(&mut self, layout: Layout) -> *mut u8 {
        self.grow();
        let align = layout.align();
        let size = layout.size();
        let aligned = (self.cursor + align - 1) & !(align - 1);
        let new_cursor = aligned + size;
        if new_cursor <= self.capacity {
            self.cursor = new_cursor;
            unsafe { self.base.add(aligned) }
        } else {
            std::ptr::null_mut()
        }
    }

    #[inline]
    fn dealloc_notify(&mut self, ptr: *mut u8, size: usize) -> bool {
        let addr = ptr as usize;

        // Fast path: active slab
        if addr >= self.active_base && addr < self.active_end {
            self.maybe_free_list(ptr, size);
            return true;
        }

        self.dealloc_notify_slow(addr, ptr, size)
    }

    #[inline]
    fn maybe_free_list(&mut self, ptr: *mut u8, size: usize) {
        if let Some(cls) = size_class_index(size) {
            if self.free_counts[cls] < MAX_FREE_PER_CLASS
                && (ptr as usize) & 7 == 0
            {
                unsafe {
                    *(ptr as *mut *mut u8) = self.free_heads[cls];
                }
                self.free_heads[cls] = ptr;
                self.free_counts[cls] += 1;
            }
        }
    }

    #[cold]
    #[inline(never)]
    fn dealloc_notify_slow(&mut self, addr: usize, ptr: *mut u8, size: usize) -> bool {
        for i in 0..self.slab_count {
            let base = self.slabs[i].base as usize;
            if addr >= base && addr < base + self.slabs[i].capacity {
                self.maybe_free_list(ptr, size);
                return true;
            }
        }

        find_global_slab(addr).is_some()
    }

    fn grow(&mut self) {
        if self.slab_count < MAX_SLABS_PER_ARENA {
            let new_size = CONFIG.arena_slab_size;
            let new_base = mmap_slab(new_size);
            if new_base.is_null() {
                return;
            }
            register_slab(new_base, new_size);
            let idx = self.slab_count;
            self.slabs[idx] = SlabMeta {
                base: new_base,
                capacity: new_size,
            };
            self.slab_count += 1;
            self.activate_slab(idx);
        }
    }

    #[inline]
    fn try_grow_in_place(&mut self, ptr: *mut u8, old_size: usize, new_size: usize) -> bool {
        let expected_end = unsafe { ptr.add(old_size) };
        let current_end = unsafe { self.base.add(self.cursor) };
        if expected_end == current_end {
            let additional = new_size - old_size;
            let new_cursor = self.cursor + additional;
            if new_cursor <= self.capacity {
                self.cursor = new_cursor;
                return true;
            }
        }
        false
    }

    fn activate_slab(&mut self, idx: usize) {
        self.active_slab = idx;
        self.base = self.slabs[idx].base;
        self.capacity = self.slabs[idx].capacity;
        self.cursor = 0;
        self.active_base = self.base as usize;
        self.active_end = self.base as usize + self.capacity;
    }
}

thread_local! {
    static ARENA_PTR: Cell<*mut WorkerArena> = const { Cell::new(std::ptr::null_mut()) };
}

#[inline]
fn with_arena<F, R>(f: F) -> R
where
    F: FnOnce(&mut WorkerArena) -> R,
{
    ARENA_PTR.with(|cell| {
        let mut ptr = cell.get();
        if ptr.is_null() {
            let size = std::mem::size_of::<WorkerArena>();
            let page_size = 4096;
            let alloc_size = (size + page_size - 1) & !(page_size - 1);
            let raw = unsafe {
                libc::mmap(
                    std::ptr::null_mut(),
                    alloc_size,
                    libc::PROT_READ | libc::PROT_WRITE,
                    libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                    -1,
                    0,
                )
            };
            if raw == libc::MAP_FAILED {
                std::process::abort();
            }
            ptr = raw as *mut WorkerArena;
            WorkerArena::init(unsafe { &mut *ptr });
            cell.set(ptr);
        }
        f(unsafe { &mut *ptr })
    })
}

pub unsafe fn alloc_small(layout: Layout) -> *mut u8 {
    let ptr = with_arena(|a| a.alloc_bump(layout));
    if ptr.is_null() {
        std::alloc::System.alloc(layout)
    } else {
        ptr
    }
}

pub unsafe fn dealloc_small(ptr: *mut u8, layout: Layout) {
    let handled = with_arena(|a| a.dealloc_notify(ptr, layout.size()));
    if !handled {
        std::alloc::System.dealloc(ptr, layout);
    }
}

pub unsafe fn alloc_medium(layout: Layout) -> *mut u8 {
    let ptr = with_arena(|a| a.alloc_bump(layout));
    if ptr.is_null() {
        std::alloc::System.alloc(layout)
    } else {
        ptr
    }
}

pub unsafe fn dealloc_medium(ptr: *mut u8, layout: Layout) {
    let handled = with_arena(|a| a.dealloc_notify(ptr, layout.size()));
    if !handled {
        std::alloc::System.dealloc(ptr, layout);
    }
}

pub unsafe fn try_grow_in_place(ptr: *mut u8, old_size: usize, new_size: usize) -> bool {
    with_arena(|a| a.try_grow_in_place(ptr, old_size, new_size))
}

pub(crate) fn compact_pools() {}
