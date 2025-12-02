use std::{collections::HashSet, hash::BuildHasher, ops::Range};

use cuckoo_clock::{
    CuckooFilter,
    config::{CuckooConfiguration, LruConfig, TtlConfig},
};

fn get_words(range: Range<usize>) -> Vec<String> {
    std::fs::read_to_string("/usr/share/dict/words")
        .unwrap()
        .split("\n")
        .skip(range.start)
        .take(range.len())
        .map(ToString::to_string)
        .collect()
}

fn run_fp_rate_test<H: BuildHasher>(
    item_count: usize,
    filter: CuckooFilter<H>,
    expected_fp_rate: f64,
) {
    assert!(item_count < 220_000);
    let words = get_words(0..item_count);
    let next_words = get_words(item_count..item_count * 2);

    let mut stored_words = HashSet::new();

    for (index, word) in words.iter().enumerate() {
        stored_words.insert(word);
        if let Some(evicted_fp) = filter.insert(word) {
            words[0..=index]
                .iter()
                .filter(|w| evicted_fp.matches_key(w, &filter))
                .for_each(|evicted_word| {
                    stored_words.remove(evicted_word);
                });
        }
    }

    for word in stored_words {
        assert!(
            filter.contains(word),
            "Word: {word} expected in the filter, but not found"
        );
    }

    let mut false_positives: u64 = 0;
    // These words were never stored, so each one reported to be in the filter is a false positive
    for word in &next_words {
        if filter.contains(word) {
            false_positives += 1;
        }
    }

    let fp_rate = false_positives as f64 / words.len() as f64;

    println!("Words count: {}", words.len());
    println!("FP rate: {}%", 100.0 * fp_rate);
    assert!(fp_rate < expected_fp_rate);
}

#[test]
fn fp_rate_default() {
    let filter = CuckooFilter::new_random(CuckooConfiguration::builder(100_000).build().unwrap());
    run_fp_rate_test(100_000, filter, 0.03);
}

#[test]
fn fp_rate_default_with_lru() {
    let filter = CuckooFilter::new_random(
        CuckooConfiguration::builder(100_000)
            .with_lru(LruConfig::default())
            .build()
            .unwrap(),
    );
    run_fp_rate_test(100_000, filter, 0.03);
}

#[test]
fn fp_rate_default_with_ttl() {
    let filter = CuckooFilter::new_random(
        CuckooConfiguration::builder(100_000)
            .with_ttl(TtlConfig {
                ttl: 10.try_into().unwrap(),
                ttl_bits: 8.try_into().unwrap(),
            })
            .build()
            .unwrap(),
    );
    run_fp_rate_test(100_000, filter, 0.03);
}

#[test]
fn fp_rate_default_with_lru_and_ttl() {
    let filter = CuckooFilter::new_random(
        CuckooConfiguration::builder(100_000)
            .with_lru(LruConfig::default())
            .with_ttl(TtlConfig {
                ttl: 10.try_into().unwrap(),
                ttl_bits: 8.try_into().unwrap(),
            })
            .build()
            .unwrap(),
    );
    run_fp_rate_test(100_000, filter, 0.03);
}

#[test]
fn fp_rate_low() {
    let filter = CuckooFilter::new_random(
        CuckooConfiguration::builder(200_000)
            .fingerprint_bits(14.try_into().unwrap())
            .build()
            .unwrap(),
    );
    run_fp_rate_test(200_000, filter, 0.001);
}

#[test]
fn fp_rate_extreme() {
    let filter = CuckooFilter::new_random(
        CuckooConfiguration::builder(200_000)
            .fingerprint_bits(32.try_into().unwrap())
            .build()
            .unwrap(),
    );
    run_fp_rate_test(200_000, filter, 0.00001);
}
