//! Tests phase boundaries that interact with rayon::scope. Workers spawned
//! inside a scope hold references to arena allocations; if begin_phase runs
//! while those workers still have pending tasks, the workers' captured data
//! could land in recycled memory.

use rayon::prelude::*;

#[global_allocator]
static A: zk_alloc::ZkAllocator = zk_alloc::ZkAllocator;

/// Phase boundary inside scope: while workers are still running, begin a
/// new phase. The worker's stack-frame data is on the worker thread's stack
/// (not arena), but any heap allocations they performed during the phase
/// could be in arena.
#[test]
fn phase_boundary_during_par_iter() {
    let _: u64 = (0..1_000_000_u64).into_par_iter().sum();

    zk_alloc::begin_phase();

    // Workers each allocate a vec, sum it. Force them to allocate in arena.
    let result: u64 = (0..16_u64).into_par_iter().map(|i| {
        let v: Vec<u64> = (0..(1 << 14)).map(|j| j ^ i).collect();
        v.iter().sum::<u64>()
    }).sum();
    std::hint::black_box(result);

    zk_alloc::end_phase();

    zk_alloc::begin_phase();
    let canary = vec![0xC9_u8; 8 << 20];
    let _: u64 = (0..16_u64).into_par_iter().map(|i| {
        let v: Vec<u64> = (0..(1 << 14)).map(|j| j ^ i).collect();
        v.iter().sum::<u64>()
    }).sum();
    zk_alloc::end_phase();

    let pos = canary.iter().position(|&b| b != 0xC9);
    assert!(
        pos.is_none(),
        "8MB canary corrupted at offset {}",
        pos.unwrap()
    );
}

/// Repeated par_iter without any explicit canary, just check program
/// integrity over 100 iterations.
#[test]
fn many_par_iter_phase_cycles() {
    let _: u64 = (0..1_000_000_u64).into_par_iter().sum();

    for _ in 0..100 {
        zk_alloc::begin_phase();
        let sum: u64 = (0..256_u64).into_par_iter().map(|i| {
            let v: Vec<u64> = (0..(1 << 12)).map(|j| j ^ i).collect();
            v.iter().sum::<u64>()
        }).sum();
        std::hint::black_box(sum);
        zk_alloc::end_phase();
    }
}
