//! Scenario 3: panic unwinding through a phase boundary.
//!
//! There is no RAII guard around begin_phase()/end_phase(). If a panic
//! propagates out of phase code without reaching end_phase(), ARENA_ACTIVE
//! stays true. Subsequent "post-phase" allocations land in arena and get
//! silently recycled on the next begin_phase().
//!
//! This is a plain API hazard: the recovery path of any prove_with_panic
//! pattern is unsafe.

#[global_allocator]
static A: zk_alloc::ZkAllocator = zk_alloc::ZkAllocator;

#[test]
fn panic_inside_phase_leaves_arena_active() {
    use std::panic;

    // Suppress default panic print to minimize incidental allocations between
    // the panic and our observation point.
    panic::set_hook(Box::new(|_| {}));
    let _ = vec![0u8; 1024]; // warm up

    zk_alloc::begin_phase();
    let r = panic::catch_unwind(panic::AssertUnwindSafe(|| panic!("simulated")));
    assert!(r.is_err());
    // No end_phase reached. ARENA_ACTIVE is still true.

    // This Vec lands in arena (since arena is still active and 8192 >=
    // MIN_ARENA_BYTES default 4096).
    let post_panic: Vec<u8> = vec![0xCC; 8192];
    let post_panic_ptr = post_panic.as_ptr() as usize;

    // Begin the next phase (e.g., next iteration of a prove loop). Arena
    // resets — anything allocated during the "ghost" phase between panic
    // and now gets recycled.
    zk_alloc::begin_phase();
    // Span enough of the slab to cover post_panic's offset, regardless of
    // how many small bumps the panic introduced.
    let big: Vec<u8> = vec![0x33; 1 << 20];
    let big_ptr = big.as_ptr() as usize;
    let big_end = big_ptr + big.len();
    zk_alloc::end_phase();

    let _ = panic::take_hook();

    let in_big_range = post_panic_ptr >= big_ptr && post_panic_ptr < big_end;
    let observed = post_panic[0];

    eprintln!(
        "post_panic_ptr=0x{post_panic_ptr:x} big=[0x{big_ptr:x}, 0x{big_end:x}); \
         in_range={in_big_range} observed=0x{observed:02x}"
    );

    assert!(
        in_big_range,
        "post-panic Vec didn't land in arena's slab — test layout assumption broken"
    );
    assert_eq!(
        observed, 0x33,
        "expected post-panic Vec contents to be recycled by next begin_phase \
         (arena was still active after the panic) — got 0x{observed:02x}"
    );
    eprintln!("BUG REPRODUCED: panic without end_phase leaves arena active; post-panic allocations recycled silently.");
}
