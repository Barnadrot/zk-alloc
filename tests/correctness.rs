use std::alloc::{GlobalAlloc, Layout};
use zk_alloc::ZkAllocator;

static ZK: ZkAllocator = ZkAllocator;

#[test]
fn small_alloc_returns_aligned_nonnull() {
    for size in [1, 8, 32, 64, 128, 256, 512] {
        let layout = Layout::from_size_align(size, 8).unwrap();
        let ptr = unsafe { ZK.alloc(layout) };
        assert!(!ptr.is_null(), "null for size {size}");
        assert_eq!(ptr as usize % 8, 0, "misaligned for size {size}");
        unsafe { ZK.dealloc(ptr, layout) };
    }
}

#[test]
fn medium_alloc_returns_aligned_nonnull() {
    for size in [1024, 2048, 4096, 65536, 1 << 20] {
        let layout = Layout::from_size_align(size, 8).unwrap();
        let ptr = unsafe { ZK.alloc(layout) };
        assert!(!ptr.is_null(), "null for size {size}");
        assert_eq!(ptr as usize % 8, 0, "misaligned for size {size}");
        unsafe { ZK.dealloc(ptr, layout) };
    }
}

#[test]
fn large_alloc_returns_aligned_nonnull() {
    for size in [4 * 1024 * 1024, 16 * 1024 * 1024, 64 * 1024 * 1024] {
        let layout = Layout::from_size_align(size, 8).unwrap();
        let ptr = unsafe { ZK.alloc(layout) };
        assert!(!ptr.is_null(), "null for size {size}");
        assert_eq!(ptr as usize % 8, 0, "misaligned for size {size}");
        unsafe { ZK.dealloc(ptr, layout) };
    }
}

#[test]
fn write_read_roundtrip() {
    let layout = Layout::from_size_align(1024, 8).unwrap();
    let ptr = unsafe { ZK.alloc(layout) };
    assert!(!ptr.is_null());

    unsafe {
        for i in 0..1024 {
            *ptr.add(i) = (i & 0xFF) as u8;
        }
        for i in 0..1024 {
            assert_eq!(*ptr.add(i), (i & 0xFF) as u8);
        }
        ZK.dealloc(ptr, layout);
    }
}

#[test]
fn zero_size_alloc_does_not_crash() {
    let layout = Layout::from_size_align(0, 1).unwrap();
    let ptr = unsafe { ZK.alloc(layout) };
    unsafe { ZK.dealloc(ptr, layout) };
}

#[test]
fn realloc_preserves_data() {
    let old_layout = Layout::from_size_align(64, 8).unwrap();
    let ptr = unsafe { ZK.alloc(old_layout) };
    assert!(!ptr.is_null());

    unsafe {
        for i in 0..64 {
            *ptr.add(i) = i as u8;
        }

        let new_ptr = ZK.realloc(ptr, old_layout, 4096);
        assert!(!new_ptr.is_null());

        for i in 0..64 {
            assert_eq!(*new_ptr.add(i), i as u8, "data corrupted at byte {i}");
        }

        let new_layout = Layout::from_size_align(4096, 8).unwrap();
        ZK.dealloc(new_ptr, new_layout);
    }
}

#[test]
fn phase_boundary_does_not_crash() {
    let layout = Layout::from_size_align(128, 8).unwrap();
    for _ in 0..10 {
        let ptr = unsafe { ZK.alloc(layout) };
        assert!(!ptr.is_null());
        unsafe { ZK.dealloc(ptr, layout) };
    }
    zk_alloc::phase_boundary();
    for _ in 0..10 {
        let ptr = unsafe { ZK.alloc(layout) };
        assert!(!ptr.is_null());
        unsafe { ZK.dealloc(ptr, layout) };
    }
}

#[test]
fn system_fallback_pointers_freed_correctly() {
    // Allocate enough to fill the 16MB arena, forcing System fallback.
    // Then dealloc the System-allocated pointers — they must not be
    // silently dropped (small) or added to our free list (medium).
    let small_layout = Layout::from_size_align(256, 8).unwrap();
    let medium_layout = Layout::from_size_align(4096, 8).unwrap();

    // Burn through arena space
    let mut arena_ptrs = Vec::new();
    for _ in 0..65536 {
        let p = unsafe { ZK.alloc(small_layout) };
        assert!(!p.is_null());
        arena_ptrs.push(p);
    }

    // These should go to System
    let sys_small = unsafe { ZK.alloc(small_layout) };
    let sys_medium = unsafe { ZK.alloc(medium_layout) };
    assert!(!sys_small.is_null());
    assert!(!sys_medium.is_null());

    // Dealloc System pointers — should not crash or corrupt
    unsafe {
        ZK.dealloc(sys_small, small_layout);
        ZK.dealloc(sys_medium, medium_layout);
    }

    // Dealloc arena pointers
    for p in arena_ptrs {
        unsafe { ZK.dealloc(p, small_layout) };
    }
}

#[test]
fn arena_grows_beyond_initial_slab() {
    // Allocate well beyond 16MB to force slab chaining.
    // Each alloc is 4KB medium, so 16MB / 4KB = 4096 fills the first slab.
    let layout = Layout::from_size_align(4096, 8).unwrap();
    let count = 8192; // ~32MB, must chain at least one new slab
    let mut ptrs = Vec::with_capacity(count);
    for _ in 0..count {
        let p = unsafe { ZK.alloc(layout) };
        assert!(!p.is_null());
        // Verify we can write to it
        unsafe { std::ptr::write_bytes(p, 0xAB, 4096) };
        ptrs.push(p);
    }
    for p in ptrs {
        unsafe { ZK.dealloc(p, layout) };
    }
}

#[test]
fn cross_thread_dealloc_does_not_crash() {
    use std::sync::mpsc;
    use std::thread;

    let small_layout = Layout::from_size_align(128, 8).unwrap();
    let medium_layout = Layout::from_size_align(2048, 8).unwrap();

    // Allocate on thread A
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let sp = unsafe { ZK.alloc(small_layout) };
        let mp = unsafe { ZK.alloc(medium_layout) };
        assert!(!sp.is_null());
        assert!(!mp.is_null());
        tx.send((sp as usize, mp as usize)).unwrap();
        // Keep thread alive briefly so its arena isn't dropped
        thread::sleep(std::time::Duration::from_millis(100));
    });

    let (sp_addr, mp_addr) = rx.recv().unwrap();
    // Dealloc on the main thread — should not crash or corrupt
    unsafe {
        ZK.dealloc(sp_addr as *mut u8, small_layout);
        ZK.dealloc(mp_addr as *mut u8, medium_layout);
    }
}

#[test]
fn medium_boundary_sizes_work() {
    // Test sizes at pool_class boundaries — especially powers of 2
    // and the maximum medium size (2MB).
    let boundary_sizes = [
        513,                // just above small threshold
        1024,               // power of 2, pool boundary
        1025,               // just above 1024
        2048,               // power of 2
        4096,               // power of 2
        65536,              // power of 2
        1 << 20,            // 1MB
        (1 << 20) + 1,      // just above 1MB
        2 * 1024 * 1024,    // 2MB = medium_threshold (max medium)
    ];
    for &size in &boundary_sizes {
        let layout = Layout::from_size_align(size, 8).unwrap();
        let ptr = unsafe { ZK.alloc(layout) };
        assert!(!ptr.is_null(), "null for size {size}");
        // Verify we can write to the full extent
        unsafe { std::ptr::write_bytes(ptr, 0xCC, size) };
        unsafe { ZK.dealloc(ptr, layout) };
    }
}

#[test]
fn phase_boundary_preserves_live_allocations() {
    // After phase_boundary, existing bump allocations must still be valid.
    let layout = Layout::from_size_align(256, 8).unwrap();
    let ptrs: Vec<*mut u8> = (0..100)
        .map(|_| unsafe {
            let p = ZK.alloc(layout);
            std::ptr::write_bytes(p, 0xAA, 256);
            p
        })
        .collect();

    zk_alloc::phase_boundary();

    // All pointers must still be readable with the data we wrote
    for &p in &ptrs {
        let val = unsafe { *p };
        assert_eq!(val, 0xAA, "data corrupted after phase_boundary");
    }

    for p in ptrs {
        unsafe { ZK.dealloc(p, layout) };
    }
}
