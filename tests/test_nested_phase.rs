//! Contract test: nested `begin_phase()` calls panic.
//!
//! Phases are flat. A nested `begin_phase()` previously corrupted the outer
//! phase's slab (later masked by a depth counter, which itself had failure
//! modes — a panic between matched calls left the depth orphaned). The
//! library now enforces the flat-phase contract with an assertion in
//! `begin_phase()`: any second begin while a phase is active is a panic.
//!
//! This test pins that behavior so a future depth-counter regression is
//! caught at `cargo test`.

#[global_allocator]
static A: zk_alloc::ZkAllocator = zk_alloc::ZkAllocator;

#[test]
#[should_panic(expected = "phases must not nest")]
fn nested_begin_phase_panics() {
    zk_alloc::begin_phase();
    // Second begin_phase while the first is still active must panic.
    zk_alloc::begin_phase();
    // Unreachable. Tearing down so a hypothetical non-panic doesn't leave
    // ARENA_ACTIVE set across other tests.
    zk_alloc::end_phase();
    zk_alloc::end_phase();
}
