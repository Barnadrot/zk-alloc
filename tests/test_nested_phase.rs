//! hunt-1: Nested phases silently corrupt outer phase data.
//!
//! Hypothesis: nested `begin_phase()` calls bump GENERATION, which marks
//! every thread's slab as needing reset. On the next allocation in the
//! inner phase, ARENA_PTR is reset to the slab base — overwriting any
//! data the outer phase had bump-allocated there.
//!
//! There is no nesting counter. The inner `end_phase()` flips
//! ARENA_ACTIVE to false, after which any allocations in the remainder
//! of the outer phase silently land in System (a different correctness
//! issue covered separately).
//!
//! Existing `tests/test_phase_guard.rs::nested_phase_guards_compose`
//! only verifies that nested guards don't panic — it never allocates
//! anything in the outer phase, so this corruption is invisible there.

#[global_allocator]
static A: zk_alloc::ZkAllocator = zk_alloc::ZkAllocator;

#[test]
fn nested_phase_does_not_corrupt_outer() {
    // Allocation big enough to land in arena (>= min_arena_bytes = 4096).
    zk_alloc::begin_phase();
    let outer: Vec<u8> = vec![0xA1_u8; 8192];
    let outer_ptr = outer.as_ptr() as usize;

    // Nested begin_phase. GENERATION bumps; the next arena alloc on
    // this thread will reset ARENA_PTR back to slab base.
    zk_alloc::begin_phase();
    let inner: Vec<u8> = vec![0xB2_u8; 8192];
    let inner_ptr = inner.as_ptr() as usize;
    let inner_first_byte = inner[0];
    zk_alloc::end_phase();

    zk_alloc::end_phase();

    eprintln!("outer_ptr=0x{outer_ptr:x} inner_ptr=0x{inner_ptr:x}");

    // If nested phases are sound, outer's bytes are still 0xA1.
    let pos = outer.iter().position(|&b| b != 0xA1);
    let _ = inner_first_byte;
    assert!(
        pos.is_none(),
        "outer phase data corrupted at offset {}: nested begin_phase bumped \
         GENERATION and the inner allocation recycled the outer's slab region",
        pos.unwrap()
    );
}
