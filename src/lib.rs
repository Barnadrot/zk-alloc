use std::alloc::{GlobalAlloc, Layout};

mod arena;
mod large;
mod phase;
mod pressure;

pub use phase::phase_boundary;

pub struct ZkAllocator;

unsafe impl GlobalAlloc for ZkAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        match layout.size() {
            0 => layout.align() as *mut u8,
            1..=512 => arena::alloc_small(layout),
            513..=2_097_152 => arena::alloc_medium(layout),
            _ => large::alloc_large(layout),
        }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        match layout.size() {
            0 => {}
            1..=512 => arena::dealloc_small(ptr, layout),
            513..=2_097_152 => arena::dealloc_medium(ptr, layout),
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
        1..=512 => 1,
        513..=2_097_152 => 2,
        _ => 3,
    }
}
