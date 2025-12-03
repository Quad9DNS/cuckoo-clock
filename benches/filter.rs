use std::hash::BuildHasher;

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use cuckoo_clock::{
    CuckooFilter,
    config::{CuckooConfiguration, LruConfig, TtlConfig},
};

fn default_configuration() -> CuckooConfiguration {
    CuckooConfiguration::builder(100_000).build().unwrap()
}

fn run_insertion_benchmark<H: BuildHasher>(
    c: &mut Criterion,
    filter: &CuckooFilter<H>,
    items: &[String],
    group_name: &str,
) {
    let mut group = c.benchmark_group(group_name);
    group.throughput(Throughput::Elements(items.len() as u64));
    group.bench_function("insert", |b| {
        b.iter(|| {
            items.iter().for_each(|item| {
                let _ = filter.insert(item);
            })
        })
    });
    group.bench_function("insert_if_not_present", |b| {
        b.iter(|| {
            items.iter().for_each(|item| {
                let _ = filter.insert_if_not_present(item);
            })
        })
    });
    group.bench_function("contains", |b| {
        b.iter(|| {
            items.iter().for_each(|item| {
                filter.contains(item);
            })
        })
    });
    group.bench_function("remove", |b| {
        b.iter(|| {
            items.iter().for_each(|item| {
                filter.remove(item);
            })
        })
    });
    group.finish();
}

fn run_scan_and_update_benchmark<H: BuildHasher>(
    c: &mut Criterion,
    filter: &CuckooFilter<H>,
    group_name: &str,
) {
    let mut group = c.benchmark_group(group_name);
    group.bench_function("scan_and_update_full", |b| {
        b.iter(|| {
            filter.scan_and_update_full();
        })
    });
    group.finish();
}

fn get_dict_words(size: usize) -> Vec<String> {
    std::fs::read_to_string("/usr/share/dict/words")
        .unwrap()
        .split("\n")
        .take(size)
        .map(ToString::to_string)
        .collect()
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

    run_insertion_benchmark(c, &filter, &items, "large_full");
    run_insertion_benchmark(c, &empty_filter, &items, "large_empty");

    let words_filter = CuckooFilter::new_random(default_configuration());
    let dict_words = get_dict_words(150_000);
    run_insertion_benchmark(c, &words_filter, &dict_words, "large_full_words");
    run_scan_and_update_benchmark(c, &filter, "large_full_scan(100_000)");
}

fn bench_large_fingeprint(c: &mut Criterion) {
    let config = CuckooConfiguration::builder(1_000_000)
        .fingerprint_bits(32.try_into().unwrap())
        .build()
        .unwrap();
    let filter = CuckooFilter::new_random(config.clone());
    let empty_filter = CuckooFilter::new_random(config);

    // Prepopulate
    (0..filter.get_bucket_count()).for_each(|i| {
        filter.insert(&format!("prepopulated_{i}"));
    });

    let item_count = 100_000;
    let mut items = Vec::with_capacity(item_count);
    (0..item_count).for_each(|i| {
        items.push(format!("item_{i}"));
    });

    run_insertion_benchmark(c, &filter, &items, "large_fingerprint_full");
    run_insertion_benchmark(c, &empty_filter, &items, "large_fingerprint_empty");
}

fn bench_large_buckets(c: &mut Criterion) {
    let config = CuckooConfiguration::builder(1_000_000)
        .fingerprint_bits(32.try_into().unwrap())
        .bucket_size(100.try_into().unwrap())
        .build()
        .unwrap();
    let filter = CuckooFilter::new_random(config.clone());
    let empty_filter = CuckooFilter::new_random(config);

    // Prepopulate
    (0..filter.get_bucket_count()).for_each(|i| {
        filter.insert(&format!("prepopulated_{i}"));
    });

    let item_count = 100_000;
    let mut items = Vec::with_capacity(item_count);
    (0..item_count).for_each(|i| {
        items.push(format!("item_{i}"));
    });

    run_insertion_benchmark(c, &filter, &items, "large_buckets_full");
    run_insertion_benchmark(c, &empty_filter, &items, "large_buckets_empty");
    run_scan_and_update_benchmark(c, &filter, "large_buckets_full_scan(1_000_000)");
}

fn bench_lru_and_ttl(c: &mut Criterion) {
    let config = CuckooConfiguration::builder(1_000_000)
        .fingerprint_bits(32.try_into().unwrap())
        .with_lru(LruConfig::default())
        .with_ttl(TtlConfig {
            ttl: 10.try_into().unwrap(),
            ttl_bits: 8.try_into().unwrap(),
        })
        .build()
        .unwrap();
    let filter = CuckooFilter::new_random(config.clone());

    // Prepopulate
    (0..filter.get_bucket_count()).for_each(|i| {
        filter.insert(&format!("prepopulated_{i}"));
    });

    let item_count = 100_000;
    let mut items = Vec::with_capacity(item_count);
    (0..item_count).for_each(|i| {
        items.push(format!("item_{i}"));
    });
    run_scan_and_update_benchmark(c, &filter, "lru_and_ttl_full_scan(1_000_000)");
}

criterion_group!(
    filter,
    bench_large,
    bench_large_fingeprint,
    bench_large_buckets,
    bench_lru_and_ttl
);
criterion_main!(filter);
