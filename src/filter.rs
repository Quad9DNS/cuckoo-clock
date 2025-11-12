use std::{
    hash::{Hash, Hasher},
    iter::repeat_with,
    marker::PhantomData,
    sync::{Arc, Mutex},
    time::Instant,
};

use crate::{
    bucket::Bucket,
    data_block::{DataBlock, DataBlockFieldConfiguration, Fingerprint},
};

#[derive(Clone)]
pub struct CuckooFilter<H> {
    configuration: CuckooConfiguration,
    derived: DerivedConfiguration,
    buckets: Arc<Vec<Mutex<Bucket>>>,
    _hasher: PhantomData<H>,
}

#[derive(Clone, Debug)]
pub struct CuckooConfiguration {
    // Max 32
    pub fingerprint_bits: usize,
    pub bucket_size: usize,
    pub max_entries: usize,
    pub max_kicks: usize,
    // LRU
    pub lru_enabled: bool,
    // TTL
    pub ttl_enabled: bool,
    pub ttl: u32,
    pub ttl_bits: usize,
    pub ttl_resolution: usize,
    // Counter
    pub counter_enabled: bool,
    pub counter_bits: usize,
}

#[derive(Clone)]
pub(crate) struct DerivedConfiguration {
    pub(crate) fingerprint_field_config: DataBlockFieldConfiguration,
    pub(crate) lru_field_config: DataBlockFieldConfiguration,
    pub(crate) counter_field_config: DataBlockFieldConfiguration,
    pub(crate) ttl_field_config: DataBlockFieldConfiguration,
    pub(crate) data_block_size: usize,
    pub(crate) bucket_count: usize,
    pub(crate) buckets_mask: u32,
}

impl DerivedConfiguration {
    pub(crate) fn derive(configuration: &CuckooConfiguration) -> Self {
        let required_bucket_count = configuration
            .max_entries
            .div_ceil(configuration.bucket_size);
        let mut bucket_count = 1;
        while bucket_count < required_bucket_count {
            bucket_count = (bucket_count + 1).next_power_of_two();
        }
        let ttl_start =
            configuration.fingerprint_bits + if configuration.lru_enabled { 8 } else { 0 };
        let counter_start = ttl_start
            + if configuration.ttl_enabled {
                configuration.ttl_bits
            } else {
                0
            };
        Self {
            fingerprint_field_config: DataBlockFieldConfiguration::new(
                0..configuration.fingerprint_bits,
            ),
            lru_field_config: DataBlockFieldConfiguration::new(
                configuration.fingerprint_bits..configuration.fingerprint_bits + 8,
            ),
            ttl_field_config: DataBlockFieldConfiguration::new(
                ttl_start..ttl_start + configuration.ttl_bits,
            ),
            counter_field_config: DataBlockFieldConfiguration::new(
                counter_start..counter_start + configuration.ttl_bits,
            ),
            data_block_size: DataBlock::get_size(configuration),
            bucket_count,
            buckets_mask: (bucket_count - 1) as u32,
        }
    }
}

impl<H> CuckooFilter<H>
where
    H: Hasher + Default,
{
    pub fn new(configuration: CuckooConfiguration) -> Self {
        let derived = DerivedConfiguration::derive(&configuration);
        let now = Instant::now();
        Self {
            configuration: configuration.clone(),
            buckets: Vec::from_iter(
                repeat_with(|| Bucket::new(&configuration, &derived, now).into())
                    .take(derived.bucket_count),
            )
            .into(),
            derived,
            _hasher: PhantomData,
        }
    }

    pub fn get_bucket_count(&self) -> usize {
        self.derived.bucket_count
    }

    pub fn insert<K: Hash + ?Sized>(&self, key: &K) {
        let (fp, i1) = self.get_fingerprint_and_index(key);
        let now = Instant::now();

        let inserted = self.buckets[i1 as usize]
            .lock()
            .expect("mutex poisoned")
            .insert(&fp, &self.configuration, &self.derived, now);

        if inserted {
            return;
        }

        let i2 = self.alt_index(&fp, i1);

        let inserted = self.buckets[i2 as usize]
            .lock()
            .expect("mutex poisoned")
            .insert(&fp, &self.configuration, &self.derived, now);

        if inserted {
            return;
        }

        let mut cur_index = i1;
        let mut data = vec![0u8; DataBlock::get_size(&self.configuration)];
        let mut cur_data_block = DataBlock::<'_>::from(&mut data[..]);
        cur_data_block.store_fingerprint(&fp, &self.derived);
        for _ in 0..self.configuration.max_kicks {
            {
                let mut bucket = self.buckets[cur_index as usize]
                    .lock()
                    .expect("mutex poisoned");
                // Replace a random item first
                if self.configuration.lru_enabled {
                    if !bucket.kick_lru(&mut cur_data_block, &self.configuration, &self.derived) {
                        return;
                    }
                } else {
                    bucket.kick_random(&mut cur_data_block, &self.configuration, &self.derived);
                }
                cur_index =
                    self.alt_index(&cur_data_block.get_fingerprint(&self.derived), cur_index);
            }

            if self.buckets[cur_index as usize]
                .lock()
                .expect("mutex poisoned")
                .insert(
                    &cur_data_block.get_fingerprint(&self.derived),
                    &self.configuration,
                    &self.derived,
                    now,
                )
            {
                // Found an alternative spot for evicted item, done with kicks
                return;
            }
        }

        // Filter is full
    }

    pub fn contains<K: Hash + ?Sized>(&self, key: &K) -> bool {
        let (fp, i1) = self.get_fingerprint_and_index(key);
        let now = Instant::now();

        let mut contains = self.buckets[i1 as usize]
            .lock()
            .expect("mutex poisoned")
            .contains(&fp, &self.configuration, &self.derived, now);

        if !contains {
            let i2 = self.alt_index(&fp, i1);
            contains = self.buckets[i2 as usize]
                .lock()
                .expect("mutex poisoned")
                .contains(&fp, &self.configuration, &self.derived, now);
        }

        contains
    }

    pub fn remove<K: Hash + ?Sized>(&self, key: &K) -> bool {
        let (fp, i1) = self.get_fingerprint_and_index(key);

        let mut removed = self.buckets[i1 as usize]
            .lock()
            .expect("mutex poisoned")
            .remove(&fp, &self.configuration, &self.derived);

        if !removed {
            let i2 = self.alt_index(&fp, i1);
            removed = self.buckets[i2 as usize]
                .lock()
                .expect("mutex poisoned")
                .remove(&fp, &self.configuration, &self.derived);
        }

        removed
    }

    fn get_fingerprint_and_index<K: Hash + ?Sized>(&self, key: &K) -> (Fingerprint, u32) {
        let mut hasher = <H as Default>::default();
        key.hash(&mut hasher);
        let result = hasher.finish();

        // Fingeprint bits over 32 are definitely an overkill
        // We can reduce number of hashes by using one hash as fingerprint and first index
        let fingerprint = (result >> 32) as u32;
        let index = result as u32 & self.derived.buckets_mask;

        (
            Fingerprint::new(
                fingerprint,
                self.derived.fingerprint_field_config.value_mask(),
            ),
            index,
        )
    }

    fn alt_index(&self, fingerprint: &Fingerprint, index: u32) -> u32 {
        let mut hasher = <H as Default>::default();
        fingerprint.hash(&mut hasher);
        let result = hasher.finish();

        (index ^ ((result as u32) & self.derived.buckets_mask)) & self.derived.buckets_mask
    }
}

#[cfg(test)]
mod tests {
    use std::hash::DefaultHasher;

    use super::*;

    fn default_configuration() -> CuckooConfiguration {
        CuckooConfiguration {
            fingerprint_bits: 8,
            bucket_size: 4,
            max_entries: 1000,
            max_kicks: 500,
            lru_enabled: false,
            ttl_enabled: false,
            ttl: 0,
            ttl_bits: 0,
            ttl_resolution: 0,
            counter_enabled: false,
            counter_bits: 0,
        }
    }

    #[test]
    fn basic_insertion() {
        let filter = CuckooFilter::<DefaultHasher>::new(default_configuration());

        filter.insert("basic");

        assert!(filter.contains("basic"));
    }

    #[test]
    fn basic_removal() {
        let filter = CuckooFilter::<DefaultHasher>::new(default_configuration());

        filter.insert("basic");

        assert!(filter.contains("basic"));

        filter.remove("basic");

        assert!(!filter.contains("basic"));
    }

    // TODO: Replace with fake hasher and hashes for more control
    #[test]
    fn lru_insertion() {
        let filter = CuckooFilter::<DefaultHasher>::new(CuckooConfiguration {
            lru_enabled: true,
            ..default_configuration()
        });

        filter.insert("test");
        filter.contains("test"); // Make it more used than others

        filter.insert("test-1"); // Sharing the same bucket as "test", but less used

        filter.insert("test8"); // Another bucket, but also valid for "test" bucket
        filter.contains("test8"); // Make it more used

        filter.insert("test25"); // Takes bucket of "test8", but less used

        // Everything fits now
        assert!(filter.contains("test"));
        assert!(filter.contains("test-1"));
        assert!(filter.contains("test8"));
        assert!(filter.contains("test25"));

        // Insert a new item which has to take one of the 2 fully occupied buckets
        filter.insert("test85");

        assert!(filter.contains("test85"));
        assert!(filter.contains("test"));
        assert!(filter.contains("test8"));

        // Either test test25 or test-1 should be missing
        assert!(
            !filter.contains("test25") || !filter.contains("test-1"),
            "No inserted items are missing, but filter can't hold them all"
        );

        // Insert both of these items again and confirm the more used ones are still there
        filter.insert("test25");
        filter.insert("test-1");
        assert!(filter.contains("test"));
        assert!(filter.contains("test8"));
    }
}
