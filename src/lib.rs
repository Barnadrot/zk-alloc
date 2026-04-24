use std::alloc::{GlobalAlloc, Layout};

mod arena;
pub mod config;
mod large;
mod phase;
mod pressure;

pub use phase::phase_boundary;

pub struct ZkAllocator;

unsafe impl GlobalAlloc for ZkAllocator {
    #[inline]
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        alloc_inner(layout)
    }

    #[inline]
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        dealloc_inner(ptr, layout);
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if new_size <= layout.size() {
            return ptr;
        }

        if layout.size() > 0
            && new_size <= config::CONFIG.medium_threshold
            && arena::try_grow_in_place(ptr, layout.size(), new_size)
        {
            return ptr;
        }

        let new_layout = Layout::from_size_align_unchecked(new_size, layout.align());
        let new_ptr = self.alloc(new_layout);
        if !new_ptr.is_null() {
            std::ptr::copy_nonoverlapping(ptr, new_ptr, layout.size());
            self.dealloc(ptr, layout);
        }
        new_ptr
    }
}

#[inline]
unsafe fn alloc_inner(layout: Layout) -> *mut u8 {
    let size = layout.size();
    match size {
        0 => layout.align() as *mut u8,
        s if s <= config::CONFIG.small_threshold => arena::alloc_small(layout),
        s if s <= config::CONFIG.medium_threshold => arena::alloc_medium(layout),
        _ => std::alloc::System.alloc(layout),
    }
}

#[inline]
unsafe fn dealloc_inner(ptr: *mut u8, layout: Layout) {
    let size = layout.size();
    match size {
        0 => {}
        s if s <= config::CONFIG.small_threshold => arena::dealloc_small(ptr, layout),
        s if s <= config::CONFIG.medium_threshold => arena::dealloc_medium(ptr, layout),
        _ => std::alloc::System.dealloc(ptr, layout),
    }
}
