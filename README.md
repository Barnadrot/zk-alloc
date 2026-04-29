# zk-alloc

Bump+reset arena allocator for ZK proving workloads.

## What it does

zk-alloc replaces the system allocator (`glibc malloc`) via Rust's `#[global_allocator]`. It `mmap`s a large virtual region with `MAP_NORESERVE` (no physical memory committed), splits it into per-thread 8GB slabs, and bumps a pointer for every allocation. `dealloc` is a no-op for arena-owned memory. Between proofs, `begin_phase()` resets all bump pointers so physical pages are reused without demand-paging costs.

## Usage

```rust
use zk_alloc::ZkAllocator;

#[global_allocator]
static ALLOC: ZkAllocator = ZkAllocator;

fn main() {
    loop {
        zk_alloc::begin_phase();             // activate arena, reset slabs
        let proof = generate_proof();        // all allocs go to arena
        zk_alloc::end_phase();               // deactivate arena
        let output = proof.clone();          // clone out before next reset
        submit(output);
    }
}
```

## Results

| Prover | Architecture | vs glibc | Mechanism |
|--------|-------------|----------|-----------|
| leanMultisig | FFT-based (Plonky3/WHIR, KoalaBear) | **-27%** warm proof | Page reuse eliminates demand-paging |
| Plonky3 | FFT-based (BabyBear, FRI) | **-12% to -17%** | Same mechanism, Poseidon1/2 and Keccak |
| Jolt | Sumcheck-based (Dory/BN254) | +1% to +4% (null) | Compute-bound; allocator overhead <1% |

FFT-based provers are memory-bound and benefit significantly. Sumcheck-based provers are compute-bound and unaffected.

## How it works

- `mmap` with `MAP_NORESERVE`: reserves virtual address space without committing physical memory
- `MADV_NOHUGEPAGE`: 4KB pages are faster for bump+reset than 2MB THP (lower per-fault cost, no compaction)
- Thread detection via `available_parallelism()`: auto-sizes to the machine
- Overflow to `System`: allocations that exceed the slab fall back to glibc
- `overflow_stats()`: reports how many allocations fell through (useful for tuning)

## Design

The technique is a 1990s bump allocator (Hanson, 1990) applied to a domain where nobody questioned malloc. The novelty is the application, not the technique.

> Hanson, D.R. (1990). "Fast allocation and deallocation of memory based on object lifetimes." Software: Practice and Experience, 20(1), 5-12.

## License

MIT
