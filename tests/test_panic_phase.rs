//! Scenario 3: panic unwinding through a phase boundary.
//!
//! There is no RAII guard around bare begin_phase()/end_phase(). If a panic
//! propagates out of phase code without reaching end_phase(), ARENA_ACTIVE
//! stays true.
//!
//! Under the flat-phase contract, the next `begin_phase()` in the recovery
//! path then panics with the "phases must not nest" message — i.e. the
//! failure is loud, not silent. `PhaseGuard` / `phase()` is the supported
//! way to make panic recovery safe; see test_phase_guard.rs.

#[global_allocator]
static A: zk_alloc::ZkAllocator = zk_alloc::ZkAllocator;

#[test]
fn panic_without_phase_guard_leaves_arena_active_and_trips_next_begin() {
    use std::panic;

    panic::set_hook(Box::new(|_| {}));
    let _ = vec![0u8; 1024]; // warm up

    zk_alloc::begin_phase();
    let r = panic::catch_unwind(panic::AssertUnwindSafe(|| panic!("simulated")));
    assert!(r.is_err());
    // No end_phase reached. ARENA_ACTIVE still true.

    // The next begin_phase must panic (flat-phase assert), surfacing the
    // missing end_phase loudly rather than silently corrupting state.
    let next = panic::catch_unwind(panic::AssertUnwindSafe(zk_alloc::begin_phase));
    let _ = panic::take_hook();
    assert!(
        next.is_err(),
        "begin_phase after an orphaned phase must panic, but it returned normally"
    );

    // Restore a clean state for subsequent tests.
    zk_alloc::end_phase();
}
