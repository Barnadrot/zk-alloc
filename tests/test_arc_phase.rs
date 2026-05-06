//! Bonus: Arc<T> retained across a phase boundary.
//!
//! Arc::new allocates ArcInner<T> = (strong_count, weak_count, T). The
//! refcounts live at the front of the allocation. If the allocation is in
//! arena (size >= MIN_ARENA_BYTES) and survives a phase boundary, the next
//! phase's first allocation overwrites the refcount fields. Subsequent
//! .clone() / drop on the Arc reads garbage refcounts → undefined behavior:
//! either a crash, a leaked allocation (refcount stays > 0), or a premature
//! "drop" if refcount happens to underflow to 1.
//!
//! Distinct from F16 because: F16 needs a realloc; Arc has fixed layout, no
//! realloc. The corruption mechanism here is plain phase-reset overwrite of
//! a still-live allocation.

use std::sync::Arc;

#[global_allocator]
static A: zk_alloc::ZkAllocator = zk_alloc::ZkAllocator;

#[test]
fn arc_strong_count_corrupted_across_phase() {
    // Big enough payload to push ArcInner over MIN_ARENA_BYTES (default 4096).
    type Payload = [u8; 16384];

    zk_alloc::begin_phase();
    let a: Arc<Payload> = Arc::new([0xAA; 16384]);
    let a_ptr = Arc::as_ptr(&a) as usize;
    zk_alloc::end_phase();

    zk_alloc::begin_phase();
    // Cover a's ArcInner location: phase reset re-bumps to slab+0; this
    // overwrites the (strong, weak, payload) layout with our bytes.
    let filler: Vec<u8> = vec![0x55; 1 << 20]; // 1 MB ensures overlap
    let filler_lo = filler.as_ptr() as usize;
    let filler_hi = filler_lo + filler.len();
    let aliased = a_ptr >= filler_lo && a_ptr < filler_hi;
    eprintln!("a_ptr=0x{a_ptr:x}, filler=[0x{filler_lo:x}, 0x{filler_hi:x}), aliased={aliased}");
    assert!(
        aliased,
        "test layout broken: filler doesn't span Arc's allocation"
    );

    // Inspect strong count via a side channel: clone() tries to fetch_add
    // on strong_count. If the refcount field has been overwritten with
    // 0x55555555..55, the clone will produce a Arc with a bogus refcount
    // and drop will not properly tear down. Worst case: SIGSEGV.
    let strong_before = Arc::strong_count(&a);
    let b = Arc::clone(&a);
    let strong_after = Arc::strong_count(&a);
    eprintln!("strong_count: before={strong_before}, after_clone={strong_after}");

    drop(b);
    drop(a);
    drop(filler);
    zk_alloc::end_phase();

    // Expect the strong count to be GARBAGE (anything other than 1 before
    // and 2 after). If it reads a valid refcount, the bug isn't manifesting
    // and the test should FAIL — either fixed or layout assumption broken.
    let pristine = strong_before == 1 && strong_after == 2;
    assert!(
        !pristine,
        "expected Arc refcount corruption (saw before={strong_before}, after={strong_after}); \
         pristine reads suggest the bug isn't firing — investigate"
    );
    eprintln!(
        "BUG CONFIRMED: Arc refcount corrupted by phase reset (before={strong_before}, after={strong_after})"
    );
}
