//! Scenario 3: panic unwinding through a phase boundary.
//!
//! There is no RAII guard around bare begin_phase()/end_phase(). If a
//! panic propagates out of phase code without reaching end_phase(),
//! PHASE_DEPTH stays > 0 and ARENA_ACTIVE stays true.
//!
//! After the depth-counter fix, this means: subsequent begin_phase() calls
//! in the recovery path nest under the orphaned phase rather than recycling
//! the slab. The slab keeps growing across "iterations" until it overflows
//! to System — a memory leak, not silent corruption. Either way the
//! recovery path is broken; PhaseGuard is the supported way to make panic
//! recovery safe (see test_phase_guard.rs).

#[global_allocator]
static A: zk_alloc::ZkAllocator = zk_alloc::ZkAllocator;

#[test]
fn panic_without_phase_guard_orphans_phase_depth() {
    use std::panic;

    // Suppress default panic print to minimize incidental allocations between
    // the panic and our observation point.
    panic::set_hook(Box::new(|_| {}));
    let _ = vec![0u8; 1024]; // warm up

    zk_alloc::begin_phase();
    let r = panic::catch_unwind(panic::AssertUnwindSafe(|| panic!("simulated")));
    assert!(r.is_err());
    // No end_phase reached. PHASE_DEPTH still 1, ARENA_ACTIVE still true.

    // Lands in arena (active=true, 8192 >= MIN_ARENA_BYTES default 4096).
    let post_panic: Vec<u8> = vec![0xCC; 8192];
    let post_panic_ptr = post_panic.as_ptr() as usize;

    // Begin the next "iteration". With the depth-counter fix this nests
    // under the orphaned phase (depth 1 → 2) instead of bumping
    // GENERATION, so the slab is NOT reset and post_panic survives.
    zk_alloc::begin_phase();
    let big: Vec<u8> = vec![0x33; 1 << 20];
    let big_ptr = big.as_ptr() as usize;
    let big_end = big_ptr + big.len();
    zk_alloc::end_phase();

    let _ = panic::take_hook();

    let post_in_big = post_panic_ptr >= big_ptr && post_panic_ptr < big_end;
    let big_overlaps_post = big_ptr < post_panic_ptr + post_panic.len()
        && big_ptr + big.len() > post_panic_ptr;
    let observed = post_panic[0];

    eprintln!(
        "post_panic_ptr=0x{post_panic_ptr:x} big=[0x{big_ptr:x}, 0x{big_end:x}); \
         post_in_big={post_in_big} big_overlaps_post={big_overlaps_post} \
         observed=0x{observed:02x}"
    );

    // With the depth-counter fix, the second begin_phase nests rather than
    // recycling. post_panic is preserved and big is allocated *after* it
    // in the same slab, so they do not overlap.
    assert!(
        !big_overlaps_post,
        "depth-counter fix failed: nested begin_phase recycled the slab and \
         big overlapped post_panic"
    );
    assert_eq!(
        observed, 0xCC,
        "post-panic Vec was corrupted; depth-counter fix should prevent the \
         next begin_phase from recycling the slab. got 0x{observed:02x}"
    );

    // Drain the orphaned depth so subsequent tests see a clean state.
    zk_alloc::end_phase();
}
