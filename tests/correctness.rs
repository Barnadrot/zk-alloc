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
