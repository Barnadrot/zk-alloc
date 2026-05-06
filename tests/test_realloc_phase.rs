//! Scenario 5: realloc across a phase boundary.
//!
//! A Vec allocated in phase N owns a pointer into the arena slab. When phase
//! N+1 begins (arena reset) and a fresh allocation lands at the same slab
//! offset, then the Vec is grown via push(), our realloc impl calls
//! alloc(new_size) and then copy_nonoverlapping(old_ptr, new_ptr,
//! old_layout.size()). The source is recycled memory — now holding the new
//! phase's data, not the original Vec's bytes. Result: the Vec's "preserved"
//! contents are silently replaced by whatever phase N+1 allocated first.
//!
//! Distinct from F1 (rayon Injector) and F4 (tracing Registry): those are
//! library-internal pooled allocations. This is a *user-visible* Vec the
//! caller deliberately holds across phases.
//!
//! The size-routing fix (MIN_ARENA_BYTES=4096) does NOT address this: any
//! Vec >= 4 KB lands in arena and inherits the bug.
//!
//! These tests assert the bug REPRODUCES (since no fix is deployed yet for
//! this case). Once a fix lands, flip the assertions.

#[global_allocator]
static A: zk_alloc::ZkAllocator = zk_alloc::ZkAllocator;

#[test]
fn realloc_across_phase_corrupts_retained_vec() {
    // Must be >= MIN_ARENA_BYTES (default 4096) so the Vec lands in arena.
    const SIZE: usize = 8192;
    const FILL: u8 = 0xAA;
    const OVERWRITE: u8 = 0x55;

    zk_alloc::begin_phase();
    let mut v: Vec<u8> = vec![FILL; SIZE];
    let v_orig_ptr = v.as_ptr() as usize;
    zk_alloc::end_phase();

    zk_alloc::begin_phase();
    // Lands at the same slab offset as v after arena reset (cold path resets
    // ARENA_PTR to ARENA_BASE on first alloc of new generation).
    let overwrite: Vec<u8> = vec![OVERWRITE; SIZE];
    let overwrite_ptr = overwrite.as_ptr() as usize;

    eprintln!(
        "v orig ptr: 0x{v_orig_ptr:x}, overwrite ptr: 0x{overwrite_ptr:x}, aliased={}",
        v_orig_ptr == overwrite_ptr
    );

    // Trigger realloc. len == cap == SIZE → push grows by doubling.
    v.push(FILL);

    let v_new_ptr = v.as_ptr() as usize;
    let first_corrupted = v[..SIZE].iter().position(|&b| b != FILL);

    zk_alloc::end_phase();
    drop(overwrite);

    eprintln!("v new ptr: 0x{v_new_ptr:x}");
    if let Some(p) = first_corrupted {
        eprintln!(
            "BUG REPRODUCED: v[{p}] = 0x{:02x} (expected 0x{FILL:02x}, got OVERWRITE 0x{OVERWRITE:02x})",
            v[p]
        );
    }

    // No fix is deployed for this case yet; bug should reproduce.
    assert!(
        first_corrupted.is_some(),
        "expected realloc-across-phase corruption (v_orig_ptr aliased overwrite_ptr=={}); \
         got pristine FILL bytes — either the bug got fixed or the layout changed",
        v_orig_ptr == overwrite_ptr
    );
}

/// Larger Vec, more conclusive. 16 KB > MIN_ARENA_BYTES default 4 KB and
/// also > most plausible threshold raises.
#[test]
fn realloc_across_phase_corrupts_large_vec() {
    const SIZE: usize = 16384;
    const FILL: u8 = 0xC1;
    const OVERWRITE: u8 = 0x3E;

    let _ = vec![0u8; 1024]; // warm up

    zk_alloc::begin_phase();
    let mut v: Vec<u8> = vec![FILL; SIZE];
    let v_orig_ptr = v.as_ptr() as usize;
    zk_alloc::end_phase();

    zk_alloc::begin_phase();
    let overwrite: Vec<u8> = vec![OVERWRITE; SIZE];
    let overwrite_ptr = overwrite.as_ptr() as usize;

    assert_eq!(
        v_orig_ptr, overwrite_ptr,
        "phase reset should re-bump ARENA_PTR to slab base, aliasing the original vec ptr"
    );

    v.push(FILL);
    let first_corrupted = v[..SIZE].iter().position(|&b| b != FILL);

    zk_alloc::end_phase();
    drop(overwrite);

    let p = first_corrupted
        .expect("expected realloc-across-phase corruption — got pristine FILL bytes (bug fixed?)");
    eprintln!(
        "confirmed: v[{p}] = 0x{:02x} (overwrite 0x{OVERWRITE:02x}, not original 0x{FILL:02x})",
        v[p]
    );
}
