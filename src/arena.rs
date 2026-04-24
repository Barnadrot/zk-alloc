use std::alloc::Layout;
use std::cell::UnsafeCell;

use crate::config::CONFIG;

const MAX_POOL_CLASSES: usize = 16;

struct BumpRegion {
    base: *mut u8,
    cursor: usize,
    capacity: usize,
}

struct FreeList {
    head: *mut FreeNode,
}

struct FreeNode {
    next: *mut FreeNode,
}

struct WorkerArena {
    bump: BumpRegion,
    pools: [FreeList; MAX_POOL_CLASSES],
    _phase_watermark: usize,
}

impl WorkerArena {
    fn new() -> Self {
        let slab_size = CONFIG.arena_slab_size;
        let base = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                slab_size,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                -1,
                0,
            ) as *mut u8
        };

        WorkerArena {
            bump: BumpRegion {
                base,
                cursor: 0,
                capacity: slab_size,
            },
            pools: [const { FreeList { head: std::ptr::null_mut() } }; MAX_POOL_CLASSES],
            _phase_watermark: 0,
        }
    }

    fn alloc_bump(&mut self, layout: Layout) -> *mut u8 {
        let align = layout.align();
        let aligned_cursor = (self.bump.cursor + align - 1) & !(align - 1);
        let new_cursor = aligned_cursor + layout.size();

        if new_cursor > self.bump.capacity {
            return std::ptr::null_mut();
        }

        self.bump.cursor = new_cursor;
        unsafe { self.bump.base.add(aligned_cursor) }
    }

    fn alloc_pool(&mut self, layout: Layout) -> *mut u8 {
        let class = pool_class(layout.size());
        let pool = &mut self.pools[class];

        if !pool.head.is_null() {
            let node = pool.head;
            pool.head = unsafe { (*node).next };
            return node as *mut u8;
        }

        let pool_size = 1 << (CONFIG.pool_base_shift + class);
        let alloc_layout =
            unsafe { Layout::from_size_align_unchecked(pool_size, layout.align().max(8)) };
        self.alloc_bump(alloc_layout)
    }

    fn dealloc_pool(&mut self, ptr: *mut u8, layout: Layout) {
        let class = pool_class(layout.size());
        let pool = &mut self.pools[class];
        let node = ptr as *mut FreeNode;
        unsafe {
            (*node).next = pool.head;
        }
        pool.head = node;
    }

    fn reset(&mut self) {
        self.bump.cursor = 0;
        self._phase_watermark = 0;
        for pool in &mut self.pools[..CONFIG.num_pool_classes] {
            pool.head = std::ptr::null_mut();
        }
    }
}

#[inline]
fn pool_class(size: usize) -> usize {
    let min_pool = 1 << CONFIG.pool_base_shift;
    let size = size.max(min_pool);
    let class = (usize::BITS - size.leading_zeros()) as usize - CONFIG.pool_base_shift;
    class.min(CONFIG.num_pool_classes - 1)
}

thread_local! {
    static ARENA: UnsafeCell<WorkerArena> = UnsafeCell::new(WorkerArena::new());
}

fn with_arena<F, R>(f: F) -> R
where
    F: FnOnce(&mut WorkerArena) -> R,
{
    ARENA.with(|cell| {
        let arena = unsafe { &mut *cell.get() };
        f(arena)
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

pub unsafe fn dealloc_small(_ptr: *mut u8, _layout: Layout) {
    // Bump allocator: individual dealloc is a no-op.
    // Memory reclaimed at phase boundary via reset().
}

pub unsafe fn alloc_medium(layout: Layout) -> *mut u8 {
    let ptr = with_arena(|a| a.alloc_pool(layout));
    if ptr.is_null() {
        std::alloc::System.alloc(layout)
    } else {
        ptr
    }
}

pub unsafe fn dealloc_medium(ptr: *mut u8, layout: Layout) {
    with_arena(|a| a.dealloc_pool(ptr, layout));
}

pub(crate) fn reset_arena() {
    with_arena(|a| a.reset());
}

use std::alloc::GlobalAlloc;
