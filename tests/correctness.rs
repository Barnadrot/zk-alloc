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
    zk_alloc::begin_phase();
    for _ in 0..10 {
        let ptr = unsafe { ZK.alloc(layout) };
        assert!(!ptr.is_null());
        unsafe { ZK.dealloc(ptr, layout) };
    }
}

#[test]
fn cross_thread_dealloc_does_not_crash() {
    use std::sync::mpsc;
    use std::thread;

    let small_layout = Layout::from_size_align(128, 8).unwrap();
    let medium_layout = Layout::from_size_align(2048, 8).unwrap();

    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let sp = unsafe { ZK.alloc(small_layout) };
        let mp = unsafe { ZK.alloc(medium_layout) };
        assert!(!sp.is_null());
        assert!(!mp.is_null());
        tx.send((sp as usize, mp as usize)).unwrap();
        thread::sleep(std::time::Duration::from_millis(100));
    });

    let (sp_addr, mp_addr) = rx.recv().unwrap();
    unsafe {
        ZK.dealloc(sp_addr as *mut u8, small_layout);
        ZK.dealloc(mp_addr as *mut u8, medium_layout);
    }
}

#[test]
fn arena_active_allocation() {
    zk_alloc::begin_phase();
    zk_alloc::begin_phase();

    let layout = Layout::from_size_align(4096, 8).unwrap();
    let mut ptrs = Vec::with_capacity(100);
    for _ in 0..100 {
        let p = unsafe { ZK.alloc(layout) };
        assert!(!p.is_null());
        assert_eq!(p as usize % 8, 0, "misaligned arena allocation");
        unsafe { std::ptr::write_bytes(p, 0xAB, 4096) };
        ptrs.push(p);
    }

    zk_alloc::end_phase();

    for p in ptrs {
        unsafe { ZK.dealloc(p, layout) };
    }
}

#[test]
fn boundary_sizes_work() {
    let sizes = [
        1,
        8,
        64,
        512,
        1024,
        2048,
        4096,
        65536,
        1 << 20,
        2 * 1024 * 1024,
        4 * 1024 * 1024,
    ];
    for &size in &sizes {
        let layout = Layout::from_size_align(size, 8).unwrap();
        let ptr = unsafe { ZK.alloc(layout) };
        assert!(!ptr.is_null(), "null for size {size}");
        unsafe { std::ptr::write_bytes(ptr, 0xCC, size) };
        unsafe { ZK.dealloc(ptr, layout) };
    }
}
