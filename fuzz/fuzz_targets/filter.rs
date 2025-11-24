#![no_main]

use std::num::{NonZeroU32, NonZeroUsize};

use cuckoo_clock::config::{LruConfig, TtlConfig};
use cuckoo_clock::{CuckooConfiguration, CuckooFilter};
use libfuzzer_sys::arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;
use libfuzzer_sys::{Corpus, arbitrary};

#[derive(Debug, Arbitrary)]
struct LruConf {
    bits: usize,
}

#[derive(Debug, Arbitrary)]
struct TtlConf {
    bits: usize,
    ttl: NonZeroU32,
}

#[derive(Debug, Arbitrary)]
struct CuckooConf {
    max_entries: usize,
    fingerprint_bits: usize,
    bucket_size: NonZeroUsize,
    lru: Option<LruConf>,
    ttl: Option<TtlConf>,
    inserts: Vec<String>,
    lookups: Vec<String>,
    deletes: Vec<String>,
}

fuzz_target!(|conf: CuckooConf| -> Corpus {
    let required_bucket_count = conf.max_entries.div_ceil(conf.bucket_size.get());
    let bucket_count = required_bucket_count.next_power_of_two();
    if bucket_count.saturating_mul(conf.bucket_size.get()) > 2_000_000_000 / 8 {
        return Corpus::Reject;
    }

    let mut config = CuckooConfiguration::builder(conf.max_entries)
        .fingerprint_bits(if let Ok(bits) = conf.fingerprint_bits.try_into() {
            bits
        } else {
            return Corpus::Reject;
        })
        .bucket_size(conf.bucket_size);

    if let Some(lru) = conf.lru {
        config = config.with_lru(LruConfig {
            counter_bits: if let Ok(bits) = lru.bits.try_into() {
                bits
            } else {
                return Corpus::Reject;
            },
        });
    }
    if let Some(ttl) = conf.ttl {
        config = config.with_ttl(TtlConfig {
            ttl: ttl.ttl,
            ttl_bits: if let Ok(bits) = ttl.bits.try_into() {
                bits
            } else {
                return Corpus::Reject;
            },
        });
    }

    let Ok(config) = config.build() else {
        return Corpus::Reject;
    };

    let filter = CuckooFilter::new_random(config);

    for insert in &conf.inserts {
        filter.insert(insert);
        assert!(filter.contains(insert));
        let data = filter.get_associated_data(insert);
        assert!(data.is_some());
        let data = data.unwrap();
        let fp = data.get_fingerprint();
        // TODO: this test probably doesn't make sense - 0 FP value representing and empty
        // fingerprint is an implementation detail
        assert!(fp != 0);
        let _ = data.get_stored_ttl_value();
        let _ = data.get_lru_counter();
        let _ = data.get_counter();
    }

    for lookup in &conf.lookups {
        filter.contains(lookup);
    }

    for delete in &conf.deletes {
        filter.remove(delete);
        assert!(!filter.contains(delete));
    }

    Corpus::Keep
});
