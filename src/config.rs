pub struct ZkAllocConfig {
    /// Initial bump arena slab size per thread (bytes).
    pub arena_slab_size: usize,

    /// Allocations <= this go to bump allocator (no individual free).
    pub small_threshold: usize,

    /// Allocations <= this go to size-class pool. Above this: mmap.
    pub medium_threshold: usize,

    /// Number of size-class pools. Pool i holds blocks of 2^(pool_base_shift + i) bytes.
    pub num_pool_classes: usize,

    /// Base shift for pool size classes. Pool 0 = 2^pool_base_shift bytes.
    pub pool_base_shift: usize,

    /// Minimum allocation size for THP madvise hint (bytes).
    pub huge_page_threshold: usize,

    /// RSS/MemAvailable percentage below which pages are returned eagerly.
    pub pressure_eager_pct: u8,

    /// RSS/MemAvailable percentage above which pages are retained aggressively.
    pub pressure_aggressive_pct: u8,
}

pub const DEFAULT: ZkAllocConfig = ZkAllocConfig {
    arena_slab_size: 16 * 1024 * 1024,  // 16MB
    small_threshold: 512,
    medium_threshold: 2 * 1024 * 1024,  // 2MB
    num_pool_classes: 12,
    pool_base_shift: 10,                // pool 0 = 1KB
    huge_page_threshold: 2 * 1024 * 1024,
    pressure_eager_pct: 50,
    pressure_aggressive_pct: 80,
};

/// Active configuration. Agent tunes these values per experiment iteration.
pub static CONFIG: ZkAllocConfig = DEFAULT;
