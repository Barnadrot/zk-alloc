//! hunt-2: realloc growth across a phase boundary can produce a destination
//! that partially overlaps the source. The current realloc uses
//! `copy_nonoverlapping`, which is UB on overlap and — with glibc's
//! forward-direction memcpy — corrupts the upper-half source bytes by
//! clobbering them before they're read.
//!
//! Construction:
//!   1. Phase N: alloc p1 at slab_base, size 8 KiB, fill with 0xC1.
//!   2. end_phase.
//!   3. Phase N+1 begin.
//!   4. First alloc in this phase resets the slab (cold path) and lands at
//!      slab_base; we make it 4 KiB and fill with 0xD0. ARENA_PTR now sits
//!      at slab_base + 4 KiB.
//!   5. Realloc p1 to grow to 16 KiB. The arena fast path returns
//!      ARENA_PTR = slab_base + 4 KiB, so src = [base, base+8K) with bytes
//!      0xD0 (first 4K, py overwrote) + 0xC1 (last 4K, untouched);
//!      dst = [base+4K, base+12K); overlap = [base+4K, base+8K) (4 KiB).
//!   6. Realloc executes `copy_nonoverlapping(src, dst, 8192)`. With glibc's
//!      forward-direction memcpy this writes src's first half (0xD0) into
//!      the overlap region, clobbering the upper half of src BEFORE it's
//!      read for dst[4K..8K]. Result: dst[4K..8K] is 0xD0 instead of 0xC1.
//!   7. With memmove (`ptr::copy`), the backward-direction copy preserves
//!      src bytes correctly: dst[4K..8K] = 0xC1.
//!
//! This test is sensitive to the platform's memcpy direction. On
//! x86_64-glibc the forward-direction implementation reliably corrupts;
//! the test passes after switching to `ptr::copy`.

use std::alloc::{GlobalAlloc, Layout};

#[global_allocator]
static A: zk_alloc::ZkAllocator = zk_alloc::ZkAllocator;

static ZK: zk_alloc::ZkAllocator = zk_alloc::ZkAllocator;

#[test]
fn realloc_partial_overlap_preserves_source_bytes() {
    // Phase N: claim slab via fresh 8 KiB alloc, fill with 0xC1.
    zk_alloc::begin_phase();
    let layout1 = Layout::from_size_align(8192, 8).unwrap();
    let p1 = unsafe { ZK.alloc(layout1) };
    assert!(!p1.is_null(), "phase-N alloc returned null");
    unsafe { std::ptr::write_bytes(p1, 0xC1, 8192) };
    zk_alloc::end_phase();

    // Phase N+1: prime ARENA_PTR by allocating 4 KiB at slab_base. This
    // runs the cold path (gen mismatch) which resets ARENA_PTR to base.
    zk_alloc::begin_phase();
    let layout_y = Layout::from_size_align(4096, 8).unwrap();
    let py = unsafe { ZK.alloc(layout_y) };
    assert!(!py.is_null(), "py alloc returned null");
    unsafe { std::ptr::write_bytes(py, 0xD0, 4096) };

    // Now realloc p1 to grow to 16 KiB. New ptr lands at slab_base + 4 KiB
    // (fast path, ARENA_PTR == base + 4K), partially overlapping p1.
    let p2 = unsafe { ZK.realloc(p1, layout1, 16384) };
    assert!(!p2.is_null(), "realloc returned null");

    println!(
        "p1=0x{:x} py=0x{:x} p2=0x{:x}",
        p1 as usize, py as usize, p2 as usize
    );

    // Read back p2's first 8 KiB: the bytes the user expects are the
    // contents of src at the time of the call, i.e. 0xD0 for [0,4K) and
    // 0xC1 for [4K,8K).
    let bytes = unsafe { std::slice::from_raw_parts(p2, 8192) };
    let lower_d0 = bytes[..4096].iter().all(|&b| b == 0xD0);
    let upper_c1 = bytes[4096..].iter().all(|&b| b == 0xC1);

    let upper_first_bad = bytes[4096..].iter().position(|&b| b != 0xC1);
    eprintln!("lower_d0={lower_d0} upper_c1={upper_c1} upper_first_bad={upper_first_bad:?}");

    assert!(
        lower_d0,
        "lower 4K of p2 should hold py's 0xD0 bytes (src had 0xD0 for [0,4K))"
    );
    assert!(
        upper_c1,
        "upper 4K of p2 was corrupted: expected 0xC1 (src's upper half) but \
         forward-direction memcpy in copy_nonoverlapping clobbered the \
         overlapping source range. Switch to ptr::copy (memmove)."
    );

    zk_alloc::end_phase();
}
