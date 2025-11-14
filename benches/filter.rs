use std::hash::BuildHasher;

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use cuckoo_clock::filter::{CuckooConfiguration, CuckooFilter};

fn default_configuration() -> CuckooConfiguration {
    CuckooConfiguration {
        fingerprint_bits: 8,
        bucket_size: 4,
        max_entries: 100_000,
        max_kicks: 500,
        lru_enabled: false,
        ttl_enabled: false,
        ttl_bits: 32,
        ttl_resolution: 1,
        ttl: 100,
        counter_enabled: false,
        counter_bits: 8,
    }
}

fn run_benchmark<H: BuildHasher>(
    c: &mut Criterion,
    filter: &CuckooFilter<H>,
    items: &[String],
    group_name: &str,
) {
    let mut full_group = c.benchmark_group(group_name);
    full_group.throughput(Throughput::Elements(items.len() as u64));
    full_group.bench_function("insert", |b| {
        b.iter(|| items.iter().for_each(|item| filter.insert(item)))
    });
    full_group.bench_function("contains", |b| {
        b.iter(|| {
            items.iter().for_each(|item| {
                filter.contains(item);
            })
        })
    });
    full_group.bench_function("remove", |b| {
        b.iter(|| {
            items.iter().for_each(|item| {
                filter.remove(item);
            })
        })
    });
    full_group.finish();
}

fn bench_large(c: &mut Criterion) {
    let filter = CuckooFilter::new_random(default_configuration());
    let empty_filter = CuckooFilter::new_random(default_configuration());

    // Prepopulate
    (0..filter.get_bucket_count()).for_each(|i| {
        filter.insert(&format!("prepopulated_{i}"));
    });

    let item_count = 100_000;
    let mut items = Vec::with_capacity(item_count);
    (0..item_count).for_each(|i| {
        items.push(format!("item_{i}"));
    });

    run_benchmark(c, &filter, &items, "large_full");
    run_benchmark(c, &empty_filter, &items, "large_empty");
}

fn bench_large_fingeprint(c: &mut Criterion) {
    let filter = CuckooFilter::new_random(CuckooConfiguration {
        fingerprint_bits: 32,
        max_entries: 1_000_000,
        ..default_configuration()
    });
    let empty_filter = CuckooFilter::new_random(CuckooConfiguration {
        fingerprint_bits: 32,
        max_entries: 1_000_000,
        ..default_configuration()
    });

    // Prepopulate
    (0..filter.get_bucket_count()).for_each(|i| {
        filter.insert(&format!("prepopulated_{i}"));
    });

    let item_count = 100_000;
    let mut items = Vec::with_capacity(item_count);
    (0..item_count).for_each(|i| {
        items.push(format!("item_{i}"));
    });

    run_benchmark(c, &filter, &items, "large_fingerprint_full");
    run_benchmark(c, &empty_filter, &items, "large_fingerprint_empty");
}

fn bench_large_buckets(c: &mut Criterion) {
    let filter = CuckooFilter::new_random(CuckooConfiguration {
        fingerprint_bits: 32,
        bucket_size: 100,
        max_entries: 1_000_000,
        ..default_configuration()
    });
    let empty_filter = CuckooFilter::new_random(CuckooConfiguration {
        fingerprint_bits: 32,
        bucket_size: 100,
        max_entries: 1_000_000,
        ..default_configuration()
    });

    // Prepopulate
    (0..filter.get_bucket_count()).for_each(|i| {
        filter.insert(&format!("prepopulated_{i}"));
    });

    let item_count = 100_000;
    let mut items = Vec::with_capacity(item_count);
    (0..item_count).for_each(|i| {
        items.push(format!("item_{i}"));
    });

    run_benchmark(c, &filter, &items, "large_buckets_full");
    run_benchmark(c, &empty_filter, &items, "large_buckets_empty");
}

criterion_group!(
    filter,
    bench_large,
    bench_large_fingeprint,
    bench_large_buckets
);
criterion_main!(filter);
