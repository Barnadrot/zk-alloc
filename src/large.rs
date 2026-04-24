use std::alloc::Layout;

use crate::config::CONFIG;

pub unsafe fn alloc_large(layout: Layout) -> *mut u8 {
    let size = layout.size();
    let ptr = libc::mmap(
        std::ptr::null_mut(),
        size,
        libc::PROT_READ | libc::PROT_WRITE,
        libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
        -1,
        0,
    ) as *mut u8;

    if ptr == libc::MAP_FAILED as *mut u8 {
        return std::ptr::null_mut();
    }

    if size >= CONFIG.huge_page_threshold {
        libc::madvise(ptr as *mut libc::c_void, size, libc::MADV_HUGEPAGE);
    }

    ptr
}

pub unsafe fn dealloc_large(ptr: *mut u8, layout: Layout) {
    libc::munmap(ptr as *mut libc::c_void, layout.size());
}
