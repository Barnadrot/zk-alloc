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
        let proof = zk_alloc::phase(|| generate_proof()); // arena on inside
        let output = proof.clone();                       // detach to System
        submit(output);
    }
}
```

`phase(|| { ... })` activates the arena, runs the closure, and deactivates
on return — including during panic unwinding (it's an RAII wrapper around
`begin_phase()` / `end_phase()`, which are also exposed for callers that
need finer-grained control).

### Two-allocator model

`ZkAllocator` routes each request to one of two backends:

- **Arena** — bump-pointer slab, used during an active phase for allocations
  ≥ `ZK_ALLOC_MIN_BYTES` (default 4096). Reset on the next `begin_phase()`.
- **System** — `glibc malloc`, used for everything else: allocations made
  outside any phase, allocations under the size-routing threshold (small
  library bookkeeping like rayon's injector blocks, tracing-subscriber
  registry slots, hashbrown HashMap entries), and `realloc` of any pointer
  that originated in System (sticky-System routing — System allocations
  never silently migrate to arena on growth).

### Phase-scoping contract

Allocations made during phase N must not be held past `begin_phase()` of
phase N+1 — that call recycles the slab, and the next allocation at the
same offset overwrites the retained bytes. **Violating this contract is
undefined behavior** (the old pointer becomes invalid the moment the
overwrite happens). In practice:

1. Drop or `clone()` arena-allocated values before the phase ends.
2. Construct long-lived state (thread pools, channels, registries) *before*
   any phase begins so it lives in System.
3. Use `phase(|| { ... })` (or a `PhaseGuard`) instead of paired calls so
   the phase ends correctly even on panic.

### Environment variables

| Variable | Default | Effect |
|----------|---------|--------|
| `ZK_ALLOC_SLAB_GB` | `8` | Per-thread slab size, in GiB. Raise for workloads that overflow (`overflow_stats()` reports the count). Total virtual reservation = `ZK_ALLOC_SLAB_GB × thread_count` (e.g., 8 GiB × 16 threads = 128 GiB virtual). Physical RAM is only consumed on touch. |
| `ZK_ALLOC_MIN_BYTES` | `4096` | Size-routing threshold. Allocations smaller than this go to System even during a phase. Set to `0` to send everything to arena (loses size-routing protection against library-internal pooled allocations). |

### Platform support

| Platform | Path | Notes |
|----------|------|-------|
| Linux x86_64 | direct syscalls (`mmap`, `madvise`) | Fastest path. No libc allocator reentrancy concerns. |
| Linux aarch64 | direct syscalls | **Requires `vm.overcommit_memory=1`** for `MAP_NORESERVE` to behave (Asahi/server-aarch64). Without it, large reservations SIGABRT. |
| Other Unix (macOS, *BSD) | libc fallback (`mmap` via libc, `madvise` no-op) | Functional, slightly slower setup; no `MADV_NOHUGEPAGE` hint. |
| Windows | no-op stubs | Allocator routes everything through System; arena is inert. Use System allocator directly here. |

Minimum RAM: at least one slab's worth (default 8 GiB) of working set per active thread when phases run. On memory-constrained machines (e.g., 16 GiB M-series Macs), set `ZK_ALLOC_SLAB_GB` lower or limit thread count.

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

Apache-2.0 — see [LICENSE](LICENSE) for the full text.
