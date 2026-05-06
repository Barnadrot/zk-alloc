//! Bonus: Box<dyn Trait> retained across phase boundary.
//!
//! A trait object is a fat pointer (data_ptr, vtable_ptr). The vtable lives
//! in .rodata (binary), so vtable lookups remain valid. The data lives in
//! the heap allocation owned by the Box. If that allocation is in arena
//! (Box payload >= MIN_ARENA_BYTES) and the Box is held across a phase,
//! the data bytes are overwritten by phase 2's first allocation.
//!
//! Method calls then read corrupted self fields. For trait impls that touch
//! pointer-typed fields (like another Box inside the struct), the corrupted
//! pointer causes SIGSEGV. For impls that only read scalar fields, the
//! corruption is silent — wrong values returned.

use std::sync::atomic::{AtomicUsize, Ordering};

#[global_allocator]
static A: zk_alloc::ZkAllocator = zk_alloc::ZkAllocator;

trait Witness {
    fn sentinel(&self) -> u64;
    fn payload_sum(&self) -> u64;
}

struct BigWitness {
    sentinel: u64,
    _pad: [u8; 16384],
    multiplier: u64,
    payload: [u64; 256],
}

impl Witness for BigWitness {
    fn sentinel(&self) -> u64 {
        self.sentinel
    }
    fn payload_sum(&self) -> u64 {
        self.payload.iter().sum::<u64>().wrapping_mul(self.multiplier)
    }
}

#[test]
fn box_dyn_trait_data_corrupted_silent() {
    let original_sum = (0..256_u64).sum::<u64>().wrapping_mul(13);

    zk_alloc::begin_phase();
    let w: Box<dyn Witness> = Box::new(BigWitness {
        sentinel: 0xDEADBEEFCAFEBABE,
        _pad: [0; 16384],
        multiplier: 13,
        payload: std::array::from_fn(|i| i as u64),
    });
    let pre_sentinel = w.sentinel();
    let pre_sum = w.payload_sum();
    assert_eq!(pre_sentinel, 0xDEADBEEFCAFEBABE);
    assert_eq!(pre_sum, original_sum);
    zk_alloc::end_phase();

    zk_alloc::begin_phase();
    let filler: Vec<u8> = vec![0x55; 1 << 20];
    std::hint::black_box(&filler);

    // Vtable lookup is fine (vtable in .rodata). Self-data fields are
    // overwritten with 0x55 bytes.
    let post_sentinel = w.sentinel();
    let post_sum = w.payload_sum();
    eprintln!(
        "before: sentinel=0x{pre_sentinel:x}, sum={pre_sum}; \
         after:  sentinel=0x{post_sentinel:x}, sum={post_sum}"
    );

    drop(filler);
    std::mem::forget(w);
    zk_alloc::end_phase();

    let pristine = post_sentinel == pre_sentinel && post_sum == pre_sum;
    assert!(
        !pristine,
        "expected Box<dyn Trait> data corruption — got pristine reads"
    );
    assert_eq!(
        post_sentinel, 0x5555555555555555,
        "sentinel should be filler bytes after phase reset"
    );
    eprintln!("BUG CONFIRMED: Box<dyn Trait> field reads return filler bytes after phase reset");
}

/// Variant: the trait method increments a global counter — confirms the
/// vtable still routes correctly even though `self` data is garbage.
#[test]
fn box_dyn_vtable_dispatch_survives_data_corruption() {
    static DISPATCH_COUNT: AtomicUsize = AtomicUsize::new(0);

    trait Counted {
        fn tick(&self);
    }
    struct BigCounter {
        _pad: [u8; 16384],
        _tag: u64,
    }
    impl Counted for BigCounter {
        fn tick(&self) {
            DISPATCH_COUNT.fetch_add(1, Ordering::Relaxed);
        }
    }

    zk_alloc::begin_phase();
    let c: Box<dyn Counted> = Box::new(BigCounter {
        _pad: [0; 16384],
        _tag: 42,
    });
    c.tick();
    zk_alloc::end_phase();

    zk_alloc::begin_phase();
    let filler: Vec<u8> = vec![0x55; 1 << 20];
    std::hint::black_box(&filler);

    // Vtable still valid; tick() runs even with corrupted self data.
    let pre = DISPATCH_COUNT.load(Ordering::Relaxed);
    c.tick();
    let post = DISPATCH_COUNT.load(Ordering::Relaxed);

    drop(filler);
    std::mem::forget(c);
    zk_alloc::end_phase();

    assert_eq!(
        post,
        pre + 1,
        "vtable dispatch should survive data corruption"
    );
    eprintln!("vtable dispatch OK across corruption (tick count {pre}→{post})");
}
