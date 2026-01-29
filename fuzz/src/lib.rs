use std::num::{NonZeroU32, NonZeroUsize};

use cuckoo_clock::config::{CuckooConfiguration, LruConfig, TtlConfig};
use libfuzzer_sys::arbitrary::{self, Arbitrary};

#[derive(Debug, Arbitrary)]
pub struct LruConf {
    pub bits: usize,
}

#[derive(Debug, Arbitrary)]
pub struct TtlConf {
    pub bits: usize,
    pub ttl: NonZeroU32,
}

#[derive(Debug, Arbitrary)]
pub struct CuckooConf {
    pub max_entries: usize,
    pub fingerprint_bits: usize,
    pub bucket_size: NonZeroUsize,
    pub lru: Option<LruConf>,
    pub ttl: Option<TtlConf>,
    pub inserts: Vec<String>,
    pub lookups: Vec<String>,
    pub deletes: Vec<String>,
}

pub fn prep_config(conf: &CuckooConf) -> Option<CuckooConfiguration> {
    let required_bucket_count = conf.max_entries.div_ceil(conf.bucket_size.get());
    let bucket_count = required_bucket_count.next_power_of_two();
    if bucket_count.saturating_mul(conf.bucket_size.get()) > 2_000_000_000 / 8 {
        return None;
    }

    let mut config = CuckooConfiguration::builder(conf.max_entries)
        .fingerprint_bits(if let Ok(bits) = conf.fingerprint_bits.try_into() {
            bits
        } else {
            return None;
        })
        .bucket_size(conf.bucket_size);

    if let Some(lru) = &conf.lru {
        config = config.with_lru(LruConfig {
            counter_bits: if let Ok(bits) = lru.bits.try_into() {
                bits
            } else {
                return None;
            },
        });
    }
    if let Some(ttl) = &conf.ttl {
        config = config.with_ttl(TtlConfig {
            ttl: ttl.ttl,
            ttl_bits: if let Ok(bits) = ttl.bits.try_into() {
                bits
            } else {
                return None;
            },
        });
    }

    let Ok(config) = config.build() else {
        return None;
    };

    Some(config)
}
