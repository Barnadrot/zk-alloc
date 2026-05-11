//! Bump-pointer arena allocator for ZK proving workloads.
//!
//! # Two-allocator model
//!
//! `ZkAllocator` is a façade over two allocators selected per call:
//!
//! - **Arena**: one `mmap` region split into per-thread slabs. Allocation
//!   bumps a thread-local pointer; `dealloc` is a no-op. `begin_phase()`
//!   resets every slab so the next phase reuses the same physical pages.
//! - **System**: `std::alloc::System` (glibc on Linux). Used for everything
//!   the arena shouldn't hold:
//!   - any allocation when no phase is active;
//!   - any allocation smaller than [`min_arena_bytes()`] even during a phase
//!     (size-routing — keeps small library bookkeeping outside the arena);
//!   - oversize allocations or threads that arrived after slabs were claimed
//!     ([`overflow_stats()`] reports these);
//!   - regrowth via `realloc` of a pointer that was already in System
//!     (sticky-System routing — System allocations don't migrate to arena
//!     on growth, even if the new size exceeds the size-routing threshold).
//!
//! # Phase scoping contract
//!
//! `begin_phase()` activates the arena and resets every slab. `end_phase()`
//! deactivates the arena. Allocations made during phase N must not be held
//! past `begin_phase()` of phase N+1: that call recycles the slab, and the
//! next allocation at the same offset will silently overwrite the retained
//! bytes.
//!
//! Practical rules:
//!
//! 1. Drop or `clone()` arena-allocated values before the phase ends.
//! 2. Use [`PhaseGuard`] / [`phase`] to ensure `end_phase` runs even on
//!    panic — without it, an unwinding phase leaves the arena active and
//!    subsequent "post-phase" allocations land in arena territory.
//! 3. Keep long-lived state (thread pools, channels, registries, caches)
//!    constructed *outside* any active phase so it lives in System.
//!
//! # Realloc migration: prevented
//!
//! `realloc` checks whether the input pointer lies in the arena region.
//! If it does, growth goes through the normal arena path (subject to
//! size-routing). If it does not, growth stays in System via
//! `System::realloc` — preventing the failure mode where a System-backed
//! `Vec` silently migrates into the arena on `push`.
//!
//! # Configuration
//!
//! - `ZK_ALLOC_SLAB_GB` — per-thread slab size in GiB (default `8`).
//! - `ZK_ALLOC_MIN_BYTES` — size-routing threshold in bytes (default `4096`).
//!   Set to `0` to send every active-phase allocation to the arena.
//!
//! # Example
//!
//! ```ignore
//! use zk_alloc::ZkAllocator;
//!
//! #[global_allocator]
//! static ALLOC: ZkAllocator = ZkAllocator;
//!
//! loop {
//!     let proof = zk_alloc::phase(|| heavy_work()); // arena on inside
//!     let output = proof.clone();                   // detach into System
//!     submit(output);
//! }
//! ```

use std::alloc::{GlobalAlloc, Layout};
use std::cell::Cell;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Once;

mod syscall;

const DEFAULT_SLAB_GB: usize = 8;
const SLACK: usize = 4;

#[derive(Debug)]
pub struct ZkAllocator;

/// Per-thread slab size in bytes. Set once during `ensure_region()` from the
/// `ZK_ALLOC_SLAB_GB` environment variable (default: 8).
static SLAB_SIZE: AtomicUsize = AtomicUsize::new(0);

/// Incremented by `begin_phase()`. Every thread caches the last value it saw in
/// `ARENA_GEN`; when they differ, the thread resets its allocation cursor to the start
/// of its slab on the next allocation. This is how a single store on the main thread
/// "resets" every other thread's slab without any cross-thread synchronization.
static GENERATION: AtomicUsize = AtomicUsize::new(0);

/// Master switch for the arena. `true` (set by `begin_phase`) routes allocations
/// through the arena; `false` (set by `end_phase`) routes them to the system allocator.
static ARENA_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Base address of the mmap'd region, or `0` before `ensure_region` runs. Read on
/// every `dealloc` to test whether a pointer belongs to us.
static REGION_BASE: AtomicUsize = AtomicUsize::new(0);

/// Total size of the mmap'd region. Set once alongside REGION_BASE.
static REGION_SIZE: AtomicUsize = AtomicUsize::new(0);

/// Synchronizes the one-time mmap so concurrent first-allocators don't race.
static REGION_INIT: Once = Once::new();

/// Monotonic counter handed out to threads to pick their slab. `fetch_add`'d once per
/// thread on its first arena allocation. Threads that get `idx >= max_threads` mark
/// themselves `ARENA_NO_SLAB` and permanently fall through to the system allocator.
static THREAD_IDX: AtomicUsize = AtomicUsize::new(0);

/// Max threads determined at init time from available_parallelism() + SLACK.
static MAX_THREADS: AtomicUsize = AtomicUsize::new(0);

static OVERFLOW_COUNT: AtomicUsize = AtomicUsize::new(0);
static OVERFLOW_BYTES: AtomicUsize = AtomicUsize::new(0);

/// Allocations smaller than this go to System even during active phases.
/// Routes registry / hashmap / injector-block-sized allocations away from
/// the arena, so library state that outlives a phase doesn't land in
/// recycled memory.
///
/// Defaults to 4096 (one page) — covers the known phase-crossing patterns:
/// crossbeam_deque::Injector blocks (~1.5 KB), tracing-subscriber Registry
/// slot data (sub-KB), hashbrown HashMap entries (sub-KB), rayon-core job
/// stack frames (sub-KB). Set ZK_ALLOC_MIN_BYTES=0 to disable, or override
/// to a different threshold.
const DEFAULT_MIN_ARENA_BYTES: usize = 4096;
static MIN_ARENA_BYTES: AtomicUsize = AtomicUsize::new(DEFAULT_MIN_ARENA_BYTES);

thread_local! {
    /// Where this thread's next allocation lands. Advanced past each allocation.
    static ARENA_PTR: Cell<usize> = const { Cell::new(0) };
    /// One past the last byte of this thread's slab.
    static ARENA_END: Cell<usize> = const { Cell::new(0) };
    /// Base address of this thread's slab (`0` = not yet claimed).
    static ARENA_BASE: Cell<usize> = const { Cell::new(0) };
    /// Last `GENERATION` value this thread observed.
    static ARENA_GEN: Cell<usize> = const { Cell::new(0) };
    /// `true` if this thread arrived after all slabs were claimed.
    static ARENA_NO_SLAB: Cell<bool> = const { Cell::new(false) };
}

fn ensure_region() -> usize {
    REGION_INIT.call_once(|| {
        let slab_gb = std::env::var("ZK_ALLOC_SLAB_GB")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(DEFAULT_SLAB_GB);
        let slab_size = slab_gb << 30;
        SLAB_SIZE.store(slab_size, Ordering::Release);

        if let Ok(s) = std::env::var("ZK_ALLOC_MIN_BYTES") {
            if let Ok(n) = s.parse::<usize>() {
                MIN_ARENA_BYTES.store(n, Ordering::Release);
            }
        }

        let cpus = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(8);
        let max_threads = cpus + SLACK;
        let region_size = slab_size * max_threads;

        // On aarch64 Linux (M2/Asahi) THP page size is 32 MiB. Over-allocate by
        // THP_SIZE so we can round REGION_BASE up to a 32 MiB boundary — required
        // for khugepaged to collapse base pages into hugepages. Without alignment
        // + an eager touch (one write per 32 MiB) the kernel collapses the touched
        // region into THP synchronously instead of relying on async khugepaged.
        #[cfg(target_arch = "aarch64")]
        const THP_SIZE: usize = 32 << 20;

        #[cfg(target_arch = "aarch64")]
        let mmap_size = region_size + THP_SIZE;
        #[cfg(not(target_arch = "aarch64"))]
        let mmap_size = region_size;
        // SAFETY: mmap_anonymous returns a page-aligned pointer or null.
        // MAP_NORESERVE means no physical memory is committed until pages are touched.
        let raw = unsafe { syscall::mmap_anonymous(mmap_size) };
        if raw.is_null() {
            std::process::abort();
        }

        #[cfg(target_arch = "aarch64")]
        let aligned_base = (raw as usize).next_multiple_of(THP_SIZE);
        #[cfg(not(target_arch = "aarch64"))]
        let aligned_base = raw as usize;

        // On aarch64, ask khugepaged to use THP for the slab region. On x86_64
        // preserve the historical NOHUGEPAGE hint (2 MiB THP can fragment slab
        // release; documented original choice).
        #[cfg(target_arch = "aarch64")]
        let advice = syscall::MADV_HUGEPAGE;
        #[cfg(not(target_arch = "aarch64"))]
        let advice = syscall::MADV_NOHUGEPAGE;
        unsafe { syscall::madvise(aligned_base as *mut u8, region_size, advice) };

        // Eager pre-touch on aarch64: write one byte per 32 MiB hugepage across
        // the first `pretouch_bytes` of every per-thread slab. Each write triggers
        // a page fault that the kernel resolves into a 32 MiB THP given our
        // MADV_HUGEPAGE hint and the 32 MiB-aligned base. Makes the THP win
        // deterministic instead of khugepaged-async-dependent.
        //
        // Adapt `pretouch_bytes` to MemTotal so total pre-touch stays under
        // MemTotal / OVERCOMMIT_GUARD (= 1/3 of RAM): on a 16 GiB Asahi M2 box,
        // a hard-coded 1 GiB × 14 slabs = 14 GiB pre-touch over-commits and gets
        // OOM-killed. Formula gives ~390 MiB per slab at 16 GiB, ~1 GiB at 64 GiB.
        // Floor at THP_SIZE so we still pre-touch at least one hugepage if
        // `total_ram_bytes()` returns 0 (stub or syscall failure).
        #[cfg(target_arch = "aarch64")]
        {
            const PRETOUCH_HARD_CAP: usize = 1 << 30;
            const OVERCOMMIT_GUARD: usize = 3;
            // SAFETY: total_ram_bytes is allocation-free on platforms with a real
            // impl, and the libc-fallback stub returns 0 without allocating.
            let mem_total = unsafe { syscall::total_ram_bytes() };
            let pretouch_bytes = if mem_total == 0 {
                THP_SIZE
            } else {
                let budget = mem_total / max_threads / OVERCOMMIT_GUARD;
                budget.clamp(THP_SIZE, PRETOUCH_HARD_CAP)
            };
            for slab_idx in 0..max_threads {
                let slab_base = aligned_base + slab_idx * slab_size;
                let mut off = 0;
                while off < pretouch_bytes {
                    // SAFETY: aligned_base..aligned_base+region_size is a valid
                    // anonymous mmap reservation; we only touch within slab.
                    unsafe {
                        std::ptr::write_volatile((slab_base + off) as *mut u8, 0);
                    }
                    off += THP_SIZE;
                }
            }
        }

        MAX_THREADS.store(max_threads, Ordering::Release);
        REGION_SIZE.store(region_size, Ordering::Release);
        REGION_BASE.store(aligned_base, Ordering::Release);
    });
    REGION_BASE.load(Ordering::Acquire)
}

/// Activates the arena and resets every thread's slab. All allocations until the next
/// `end_phase()` go to the arena; the previous phase's data is overwritten in place.
///
/// ## Phases must not nest
///
/// Calling `begin_phase()` while another phase is already active panics. The
/// arena is a flat lifetime — nested phases were previously tolerated via a
/// depth counter, but the depth counter masked correctness bugs (panics
/// orphaning the count, accidental double-begin recycling the outer phase's
/// slab on the next allocation). The contract is now: every `begin_phase()`
/// is paired with one `end_phase()` (or use [`PhaseGuard`] / [`phase`] for
/// panic-safe pairing), and no second `begin_phase()` is reachable from
/// within an active phase.
///
/// ## Retention is unsafe
///
/// Allocations made during phase N that are still held when phase N+1 begins
/// are silently overwritten by phase N+1's first allocations at the same slab
/// offset. Any of the following held across `begin_phase()` will be corrupted:
///
/// - `Vec<T>` with capacity ≥ [`min_arena_bytes()`] (`push` triggers `realloc`
///   that copies from now-recycled source memory).
/// - `Arc<T>` / `Rc<T>` with payload ≥ [`min_arena_bytes()`] (refcount fields
///   become arbitrary bytes — silent leak or use-after-free).
/// - `HashMap`, `BTreeMap`, etc. with bucket allocation ≥ [`min_arena_bytes()`]
///   (lookup may infinite-loop on corrupted ctrl bytes).
/// - `Box<dyn Trait>` with backing data ≥ [`min_arena_bytes()`] (vtable
///   dispatch survives but field reads return filler bytes).
///
/// To preserve data across phases, `clone()` it into a System-backed copy
/// (e.g., wrap in `Box::leak(Box::new(...))` while ARENA_ACTIVE is false,
/// or copy into a `Vec` allocated outside any phase).
pub fn begin_phase() {
    ensure_region();
    let prev_active = ARENA_ACTIVE.swap(true, Ordering::Release);
    assert!(
        !prev_active,
        "begin_phase() called while another phase is already active — phases must not nest"
    );
    GENERATION.fetch_add(1, Ordering::Release);
}

/// Deactivates the arena. New allocations go to the system allocator; existing arena
/// pointers stay valid until the next `begin_phase()` resets the slabs.
///
/// With the `rayon-flush` feature (default), this also drains rayon's internal
/// queues to release any crossbeam-deque blocks allocated during the phase.
///
/// Idempotent: calling `end_phase()` while no phase is active is a no-op.
pub fn end_phase() {
    ARENA_ACTIVE.store(false, Ordering::Release);
    #[cfg(feature = "rayon-flush")]
    flush_rayon();
}

/// Drains rayon's crossbeam-deque injector to release blocks allocated during
/// the active phase. Without this, `begin_phase()` would recycle memory that
/// rayon's injector still references, causing silent corruption.
///
/// Pushes `FLUSH_JOBS` no-op joins. Each consumes one injector slot; once a
/// block's last slot is consumed, crossbeam deallocates it. The fresh tail
/// block lands in the system allocator (arena is already inactive).
#[cfg(feature = "rayon-flush")]
fn flush_rayon() {
    const FLUSH_JOBS: usize = 256;
    for _ in 0..FLUSH_JOBS {
        rayon::join(|| {}, || {});
    }
}

/// RAII guard for an arena phase. Calls `begin_phase()` on construction and
/// `end_phase()` on drop — including during panic unwinding. Use this in
/// place of paired `begin_phase()`/`end_phase()` calls when the phase body
/// can panic, to avoid leaving the arena active across the unwind.
///
/// ```ignore
/// loop {
///     let _guard = zk_alloc::PhaseGuard::new();
///     heavy_work_that_might_panic();
///     // _guard drops here on normal return AND on unwind
/// }
/// ```
pub struct PhaseGuard {
    _private: (),
}

impl PhaseGuard {
    /// Begins a phase. The phase ends when the returned guard is dropped.
    pub fn new() -> Self {
        begin_phase();
        Self { _private: () }
    }
}

impl Default for PhaseGuard {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for PhaseGuard {
    fn drop(&mut self) {
        end_phase();
    }
}

/// Runs `f` inside a phase. Equivalent to constructing a `PhaseGuard`,
/// running `f`, and dropping the guard. Panics in `f` propagate, but the
/// phase is guaranteed to end before unwinding leaves this function.
pub fn phase<F, R>(f: F) -> R
where
    F: FnOnce() -> R,
{
    let _guard = PhaseGuard::new();
    f()
}

/// Returns (overflow_count, overflow_bytes) — allocations that fell through to System
/// because they exceeded the slab or arrived after all slabs were claimed.
pub fn overflow_stats() -> (usize, usize) {
    (
        OVERFLOW_COUNT.load(Ordering::Relaxed),
        OVERFLOW_BYTES.load(Ordering::Relaxed),
    )
}

pub fn reset_overflow_stats() {
    OVERFLOW_COUNT.store(0, Ordering::Relaxed);
    OVERFLOW_BYTES.store(0, Ordering::Relaxed);
}

/// Returns the per-thread slab size in bytes. Zero before the first `begin_phase()`.
pub fn slab_size() -> usize {
    SLAB_SIZE.load(Ordering::Relaxed)
}

/// Returns the minimum allocation size routed through the arena. Allocations
/// smaller than this go to System even during active phases.
pub fn min_arena_bytes() -> usize {
    MIN_ARENA_BYTES.load(Ordering::Relaxed)
}

#[cold]
#[inline(never)]
unsafe fn arena_alloc_cold(size: usize, align: usize) -> *mut u8 {
    let generation = GENERATION.load(Ordering::Relaxed);
    if !ARENA_NO_SLAB.get() && ARENA_GEN.get() != generation {
        let mut base = ARENA_BASE.get();
        if base == 0 {
            let region = ensure_region();
            let max = MAX_THREADS.load(Ordering::Relaxed);
            let idx = THREAD_IDX.fetch_add(1, Ordering::Relaxed);
            if idx >= max {
                ARENA_NO_SLAB.set(true);
                return unsafe {
                    std::alloc::System.alloc(Layout::from_size_align_unchecked(size, align))
                };
            }
            let slab_size = SLAB_SIZE.load(Ordering::Relaxed);
            base = region + idx * slab_size;
            ARENA_BASE.set(base);
            ARENA_END.set(base + slab_size);
        }
        ARENA_PTR.set(base);
        ARENA_GEN.set(generation);
        let aligned = (base + align - 1) & !(align - 1);
        let new_ptr = aligned + size;
        if new_ptr <= ARENA_END.get() {
            ARENA_PTR.set(new_ptr);
            return aligned as *mut u8;
        }
    }
    OVERFLOW_COUNT.fetch_add(1, Ordering::Relaxed);
    OVERFLOW_BYTES.fetch_add(size, Ordering::Relaxed);
    unsafe { std::alloc::System.alloc(Layout::from_size_align_unchecked(size, align)) }
}

// SAFETY: All pointers returned are either from our mmap'd region (valid, aligned,
// non-overlapping per thread) or from System. The arena is thread-local so no data
// races. Relaxed ordering on ARENA_ACTIVE/GENERATION is sound: worst case a thread
// sees a stale value and does one extra system-alloc before picking up the new
// generation on the next call.
unsafe impl GlobalAlloc for ZkAllocator {
    #[inline(always)]
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if ARENA_ACTIVE.load(Ordering::Relaxed) {
            // Small allocs bypass arena: registry slots / HashMap entries /
            // injector-block-sized allocations from rayon/tracing libraries
            // commonly outlive a phase. Routing them to System keeps them
            // safe across begin_phase()/end_phase() boundaries.
            let min_bytes = MIN_ARENA_BYTES.load(Ordering::Relaxed);
            if min_bytes != 0 && layout.size() < min_bytes {
                return unsafe { std::alloc::System.alloc(layout) };
            }
            let generation = GENERATION.load(Ordering::Relaxed);
            if ARENA_GEN.get() == generation {
                let ptr = ARENA_PTR.get();
                let aligned = (ptr + layout.align() - 1) & !(layout.align() - 1);
                let new_ptr = aligned + layout.size();
                if new_ptr <= ARENA_END.get() {
                    ARENA_PTR.set(new_ptr);
                    return aligned as *mut u8;
                }
            }
            return unsafe { arena_alloc_cold(layout.size(), layout.align()) };
        }
        unsafe { std::alloc::System.alloc(layout) }
    }

    #[inline(always)]
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        let addr = ptr as usize;
        let base = REGION_BASE.load(Ordering::Relaxed);
        let region_size = REGION_SIZE.load(Ordering::Relaxed);
        if base != 0 && addr >= base && addr < base + region_size {
            return;
        }
        unsafe { std::alloc::System.dealloc(ptr, layout) };
    }

    #[inline(always)]
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if new_size <= layout.size() {
            return ptr;
        }
        // Sticky-System routing: if the original allocation came from System
        // (small, or pre-phase, or routed by size-routing), keep the grown
        // allocation in System too. Without this, a Vec allocated outside
        // a phase that grows inside one would silently migrate into the
        // arena and become subject to phase recycling.
        let addr = ptr as usize;
        let base = REGION_BASE.load(Ordering::Relaxed);
        let region_size = REGION_SIZE.load(Ordering::Relaxed);
        let in_arena = base != 0 && addr >= base && addr < base + region_size;
        if !in_arena {
            return unsafe { std::alloc::System.realloc(ptr, layout, new_size) };
        }
        let new_layout = unsafe { Layout::from_size_align_unchecked(new_size, layout.align()) };
        let new_ptr = unsafe { self.alloc(new_layout) };
        if !new_ptr.is_null() {
            // Use `ptr::copy` (memmove) instead of `copy_nonoverlapping`:
            // when reallocating an arena pointer across a phase boundary,
            // the cold-path slab reset (or fast-path bump after reset) can
            // hand back a pointer that aliases or partially overlaps the
            // source. `copy_nonoverlapping` is UB on overlap; `copy`
            // handles it correctly. Modern x86_64 memcpy implementations
            // happen to be safe for short overlaps in practice, but the
            // language-level UB is real and would surface under miri or
            // future codegen.
            unsafe { std::ptr::copy(ptr, new_ptr, layout.size()) };
            unsafe { self.dealloc(ptr, layout) };
        }
        new_ptr
    }
}
