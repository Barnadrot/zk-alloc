use criterion::{criterion_group, criterion_main, Criterion};
use std::alloc::{GlobalAlloc, Layout};

use zk_alloc::ZkAllocator;

static ZK: ZkAllocator = ZkAllocator;

fn bench_small_alloc(c: &mut Criterion) {
    let layout = Layout::from_size_align(64, 8).unwrap();
    c.bench_function("small_alloc_dealloc_64B", |b| {
        b.iter(|| unsafe {
            let ptr = ZK.alloc(layout);
            std::hint::black_box(ptr);
            ZK.dealloc(ptr, layout);
        });
    });
}

fn bench_medium_alloc(c: &mut Criterion) {
    let layout = Layout::from_size_align(4096, 8).unwrap();
    c.bench_function("medium_alloc_dealloc_4KB", |b| {
        b.iter(|| unsafe {
            let ptr = ZK.alloc(layout);
            std::hint::black_box(ptr);
            ZK.dealloc(ptr, layout);
        });
    });
}

fn bench_large_alloc(c: &mut Criterion) {
    let layout = Layout::from_size_align(4 * 1024 * 1024, 8).unwrap();
    c.bench_function("large_alloc_dealloc_4MB", |b| {
        b.iter(|| unsafe {
            let ptr = ZK.alloc(layout);
            std::hint::black_box(ptr);
            ZK.dealloc(ptr, layout);
        });
    });
}

fn bench_arena_bump(c: &mut Criterion) {
    zk_alloc::begin_phase();
    let layout = Layout::from_size_align(64, 8).unwrap();
    c.bench_function("arena_bump_64B", |b| {
        b.iter(|| unsafe {
            let ptr = ZK.alloc(layout);
            std::hint::black_box(ptr);
        });
    });
    zk_alloc::end_phase();
}

criterion_group!(
    benches,
    bench_small_alloc,
    bench_medium_alloc,
    bench_large_alloc,
    bench_arena_bump,
);
criterion_main!(benches);
