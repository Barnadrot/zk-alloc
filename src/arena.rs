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
static ARENA_ADDR_MIN: AtomicUsize = AtomicUsize::new(usize::MAX);
static ARENA_ADDR_MAX: AtomicUsize = AtomicUsize::new(0);

fn register_slab(base: *mut u8, capacity: usize) -> usize {
    let idx = SLAB_COUNT.fetch_add(1, Ordering::Relaxed);
    if idx >= MAX_GLOBAL_SLABS {
        std::process::abort();
    }
    SLAB_BASES[idx].store(base as usize, Ordering::Relaxed);
    SLAB_ENDS[idx].store(base as usize + capacity, Ordering::Release);
    let addr = base as usize;
    update_min(&ARENA_ADDR_MIN, addr);
    update_max(&ARENA_ADDR_MAX, addr + capacity);
    idx
}

fn update_min(atom: &AtomicUsize, val: usize) {
    let mut cur = atom.load(Ordering::Relaxed);
    while val < cur {
        match atom.compare_exchange_weak(cur, val, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => break,
            Err(v) => cur = v,
        }
    }
}

fn update_max(atom: &AtomicUsize, val: usize) {
    let mut cur = atom.load(Ordering::Relaxed);
    while val > cur {
        match atom.compare_exchange_weak(cur, val, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => break,
            Err(v) => cur = v,
        }
    }
}

#[inline]
fn is_any_arena(addr: usize) -> bool {
    addr >= ARENA_ADDR_MIN.load(Ordering::Relaxed)
        && addr < ARENA_ADDR_MAX.load(Ordering::Relaxed)
}

fn mmap_slab(size: usize) -> *mut u8 {
    let ptr = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            size,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_PRIVATE | libc::MAP_ANONYMOUS | libc::MAP_HUGETLB,
            -1,
            0,
        )
    };
    if ptr != libc::MAP_FAILED {
        return ptr as *mut u8;
    }
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
        return std::ptr::null_mut();
    }
    unsafe {
        libc::madvise(ptr, size, libc::MADV_HUGEPAGE);
    }
    ptr as *mut u8
}

const NUM_SIZE_CLASSES: usize = 10;
const SIZE_CLASSES: [usize; NUM_SIZE_CLASSES] = [8, 16, 32, 64, 128, 256, 512, 1024, 2048, 4096];

#[inline]
fn size_class_index(size: usize) -> Option<usize> {
    if size <= 8 { return Some(0); }
    if size <= 16 { return Some(1); }
    if size <= 32 { return Some(2); }
    if size <= 64 { return Some(3); }
    if size <= 128 { return Some(4); }
    if size <= 256 { return Some(5); }
    if size <= 512 { return Some(6); }
    if size <= 1024 { return Some(7); }
    if size <= 2048 { return Some(8); }
    if size <= 4096 { return Some(9); }
    None
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
    free_counts: [u32; NUM_SIZE_CLASSES],

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

        if let Some(cls) = size_class_index(size) {
            let head = self.free_heads[cls];
            if !head.is_null() && (head as usize) & (align - 1) == 0 {
                self.free_heads[cls] = unsafe { *(head as *const *mut u8) };
                self.free_counts[cls] -= 1;
                return head;
            }
            // Bump-allocate the class size for free-list consistency
            let class_size = SIZE_CLASSES[cls];
            let aligned = (self.cursor + align - 1) & !(align - 1);
            let new_cursor = aligned + class_size;
            if new_cursor <= self.capacity {
                self.cursor = new_cursor;
                return unsafe { self.base.add(aligned) };
            }
            return self.alloc_bump_slow_class(align, class_size);
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
    fn alloc_bump_slow_class(&mut self, align: usize, class_size: usize) -> *mut u8 {
        self.grow();
        let aligned = (self.cursor + align - 1) & !(align - 1);
        let new_cursor = aligned + class_size;
        if new_cursor <= self.capacity {
            self.cursor = new_cursor;
            unsafe { self.base.add(aligned) }
        } else {
            std::ptr::null_mut()
        }
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

        if addr >= self.active_base && addr < self.active_end {
            self.maybe_free_list(ptr, size);
            return true;
        }

        self.dealloc_notify_slow(addr, ptr, size)
    }

    #[inline]
    fn maybe_free_list(&mut self, ptr: *mut u8, size: usize) {
        if let Some(cls) = size_class_index(size) {
            if self.free_counts[cls] < 65536
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
    fn dealloc_notify_slow(&mut self, addr: usize, _ptr: *mut u8, _size: usize) -> bool {
        for i in 0..self.slab_count {
            let base = self.slabs[i].base as usize;
            if addr >= base && addr < base + self.slabs[i].capacity {
                return true;
            }
        }

        is_any_arena(addr)
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
