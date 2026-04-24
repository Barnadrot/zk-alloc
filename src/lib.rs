use std::alloc::{GlobalAlloc, Layout};

mod arena;
pub mod config;
mod large;
mod phase;
mod pressure;

pub use phase::phase_boundary;

pub struct ZkAllocator;

unsafe impl GlobalAlloc for ZkAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let size = layout.size();
        match size {
            0 => layout.align() as *mut u8,
            s if s <= config::CONFIG.small_threshold => arena::alloc_small(layout),
            s if s <= config::CONFIG.medium_threshold => arena::alloc_medium(layout),
            _ => large::alloc_large(layout),
        }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        let size = layout.size();
        match size {
            0 => {}
            s if s <= config::CONFIG.small_threshold => arena::dealloc_small(ptr, layout),
            s if s <= config::CONFIG.medium_threshold => arena::dealloc_medium(ptr, layout),
            _ => large::dealloc_large(ptr, layout),
        }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let old_class = size_class(layout.size());
        let new_class = size_class(new_size);

        if old_class == new_class {
            return ptr;
        }

        let new_layout = Layout::from_size_align_unchecked(new_size, layout.align());
        let new_ptr = self.alloc(new_layout);
        if !new_ptr.is_null() {
            std::ptr::copy_nonoverlapping(ptr, new_ptr, layout.size().min(new_size));
            self.dealloc(ptr, layout);
        }
        new_ptr
    }
}

#[inline]
fn size_class(size: usize) -> u8 {
    match size {
        0 => 0,
        s if s <= config::CONFIG.small_threshold => 1,
        s if s <= config::CONFIG.medium_threshold => 2,
        _ => 3,
    }
}
