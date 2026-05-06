//! Verify that PhaseGuard / phase() makes F17 (panic leaves arena active)
//! impossible by construction. Drop runs during unwind, calling end_phase.
//!
//! Mirrors test_panic_phase but uses the RAII API. Asserts NO corruption.
//!
//! All three tests in this binary touch the global ARENA_ACTIVE / bump
//! pointer state, so they must not run concurrently — the panic-handler
//! hook is also process-global. Serialize via a file-local mutex.

static PHASE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[global_allocator]
static A: zk_alloc::ZkAllocator = zk_alloc::ZkAllocator;

#[test]
fn phase_guard_runs_end_phase_on_panic() {
    let _lock = PHASE_LOCK.lock().unwrap();
    use std::panic;

    panic::set_hook(Box::new(|_| {}));
    let _ = vec![0u8; 1024];

    // Mirror of test_panic_phase::panic_inside_phase_leaves_arena_active.
    // Use phase() / PhaseGuard around the panic — the guard's Drop ends
    // the phase during unwind.
    let r = panic::catch_unwind(panic::AssertUnwindSafe(|| {
        zk_alloc::phase(|| panic!("simulated"))
    }));
    assert!(r.is_err());

    // Arena should now be inactive — this large allocation should land in
    // System, not arena.
    let post_panic: Vec<u8> = vec![0xCC; 8192];
    let post_panic_ptr = post_panic.as_ptr() as usize;

    // Begin a new phase + 1 MB filler. If the previous phase was correctly
    // ended, post_panic is in System and won't be recycled. The filler
    // lands somewhere in arena slab+0 — but post_panic_ptr is NOT in arena.
    zk_alloc::phase(|| {
        let big: Vec<u8> = vec![0x33; 1 << 20];
        let big_ptr = big.as_ptr() as usize;
        let big_end = big_ptr + big.len();
        let in_big_range = post_panic_ptr >= big_ptr && post_panic_ptr < big_end;
        eprintln!(
            "post_panic_ptr=0x{post_panic_ptr:x} big=[0x{big_ptr:x}, 0x{big_end:x}) \
             in_range={in_big_range}"
        );
        // post_panic should NOT be in arena range (it was allocated when
        // ARENA_ACTIVE=false because PhaseGuard's Drop ran during the unwind).
        assert!(
            !in_big_range,
            "PhaseGuard didn't run end_phase during unwind — post_panic landed in arena"
        );
    });

    let _ = panic::take_hook();

    // Verify post_panic's contents are pristine.
    assert!(
        post_panic.iter().all(|&b| b == 0xCC),
        "post_panic was corrupted; PhaseGuard didn't end the phase on panic"
    );
    eprintln!("PhaseGuard fix verified: panic unwound through phase, end_phase ran, post-panic Vec safe in System");
}

#[test]
fn phase_guard_runs_end_phase_on_normal_return() {
    let _lock = PHASE_LOCK.lock().unwrap();
    let v = zk_alloc::phase(|| vec![0xAB_u8; 8192]);
    // After phase, arena is inactive. Subsequent allocations go to System.
    let after: Vec<u8> = vec![0xCD_u8; 8192];

    // Begin another phase + filler. `after` should not be recycled (it's in System).
    zk_alloc::phase(|| {
        let _filler: Vec<u8> = vec![0x77_u8; 1 << 20];
    });

    assert!(
        after.iter().all(|&b| b == 0xCD),
        "after-phase Vec was corrupted"
    );
    // v is in arena from the first phase; it MAY be corrupted by phase 2.
    // That's the F16 family — not what this test is about. We don't assert
    // on v.
    std::hint::black_box(v);
}

#[test]
fn nested_phase_guards_compose() {
    let _lock = PHASE_LOCK.lock().unwrap();
    // Outer phase + inner phase. Inner phase end_phases (sets active=false),
    // then outer phase end_phases again. Sequence: begin, begin, end, end.
    // Final state: active=false. No panic.
    let result = zk_alloc::phase(|| zk_alloc::phase(|| 42_u64));
    assert_eq!(result, 42);
}
