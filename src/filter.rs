use std::{
    hash::{BuildHasher, Hash, RandomState},
    iter::repeat_with,
    sync::{Arc, Mutex},
    time::Instant,
};

use crate::{
    associated_data::AssociatedData,
    bucket::Bucket,
    config::CuckooConfiguration,
    data_block::{DataBlock, Fingerprint},
};

#[derive(Clone)]
pub struct CuckooFilter<H: BuildHasher> {
    configuration: CuckooConfiguration,
    buckets: Arc<Vec<Mutex<Bucket>>>,
    build_hasher: H,
}

impl CuckooFilter<RandomState> {
    #[must_use]
    pub fn new_random(configuration: CuckooConfiguration) -> Self {
        Self::new(configuration, RandomState::new())
    }
}

#[allow(clippy::expect_used)]
impl<H: BuildHasher> CuckooFilter<H> {
    pub fn new(configuration: CuckooConfiguration, build_hasher: H) -> Self {
        let now = Instant::now();
        Self {
            configuration: configuration.clone(),
            buckets: repeat_with(|| Bucket::new(&configuration, now).into())
                .take(configuration.bucket_count)
                .collect::<Vec<_>>()
                .into(),
            build_hasher,
        }
    }

    pub fn get_bucket_count(&self) -> usize {
        self.configuration.bucket_count
    }

    pub fn insert<K: Hash + ?Sized>(&self, key: &K) {
        let (fp, i1) = self.get_fingerprint_and_index(key);
        let now = Instant::now();

        let inserted = self.buckets[i1 as usize]
            .lock()
            .expect("mutex poisoned")
            .insert(&fp, &self.configuration, now);

        if inserted {
            return;
        }

        let i2 = self.alt_index(&fp, i1);

        let inserted = self.buckets[i2 as usize]
            .lock()
            .expect("mutex poisoned")
            .insert(&fp, &self.configuration, now);

        if inserted {
            return;
        }

        let mut cur_index = i1;
        let mut data = vec![0u8; self.configuration.data_block_size];
        let mut cur_data_block = DataBlock::<'_>::from(&mut data[..]);
        cur_data_block.store_fingerprint(&fp, &self.configuration);
        for _ in 0..self.configuration.max_kicks {
            {
                let mut bucket = self.buckets[cur_index as usize]
                    .lock()
                    .expect("mutex poisoned");
                // Replace a random item first
                if let Some(lru_config) = self.configuration.lru_field_config.as_ref() {
                    if !bucket.kick_lru(&mut cur_data_block, &self.configuration, lru_config) {
                        return;
                    }
                } else {
                    bucket.kick_random(&mut cur_data_block, &self.configuration);
                }
                cur_index = self.alt_index(
                    &cur_data_block.get_fingerprint(&self.configuration),
                    cur_index,
                );
            }

            if self.buckets[cur_index as usize]
                .lock()
                .expect("mutex poisoned")
                .insert(
                    &cur_data_block.get_fingerprint(&self.configuration),
                    &self.configuration,
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
            .contains(&fp, &self.configuration, now);

        if !contains {
            let i2 = self.alt_index(&fp, i1);
            contains = self.buckets[i2 as usize]
                .lock()
                .expect("mutex poisoned")
                .contains(&fp, &self.configuration, now);
        }

        contains
    }

    pub fn get_associated_data<K: Hash + ?Sized>(&self, key: &K) -> Option<AssociatedData> {
        let (fp, i1) = self.get_fingerprint_and_index(key);
        let now = Instant::now();

        let mut contains = self.buckets[i1 as usize]
            .lock()
            .expect("mutex poisoned")
            .get_associated_data(&fp, &self.configuration, now);

        if contains.is_none() {
            let i2 = self.alt_index(&fp, i1);
            contains = self.buckets[i2 as usize]
                .lock()
                .expect("mutex poisoned")
                .get_associated_data(&fp, &self.configuration, now);
        }

        contains
    }

    pub fn remove<K: Hash + ?Sized>(&self, key: &K) -> bool {
        let (fp, i1) = self.get_fingerprint_and_index(key);

        let mut removed = self.buckets[i1 as usize]
            .lock()
            .expect("mutex poisoned")
            .remove(&fp, &self.configuration);

        if !removed {
            let i2 = self.alt_index(&fp, i1);
            removed = self.buckets[i2 as usize]
                .lock()
                .expect("mutex poisoned")
                .remove(&fp, &self.configuration);
        }

        removed
    }

    fn get_fingerprint_and_index<K: Hash + ?Sized>(&self, key: &K) -> (Fingerprint, u32) {
        let result = self.build_hasher.hash_one(key);

        // Fingeprint bits over 32 are definitely an overkill
        // We can reduce number of hashes by using one hash as fingerprint and first index
        let fingerprint = (result >> 32) as u32;
        // Intentional truncation here
        #[allow(clippy::cast_possible_truncation)]
        let index = result as u32 & self.configuration.buckets_mask;

        (
            Fingerprint::new(
                fingerprint,
                self.configuration.fingerprint_field_config.value_mask(),
            ),
            index,
        )
    }

    // Intentional truncation here
    #[allow(clippy::cast_possible_truncation)]
    fn alt_index(&self, fingerprint: &Fingerprint, index: u32) -> u32 {
        let result = self.build_hasher.hash_one(fingerprint);

        (index ^ ((result as u32) & self.configuration.buckets_mask))
            & self.configuration.buckets_mask
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use crate::config::LruConfig;

    use super::*;

    #[test]
    fn basic_insertion() {
        let filter = CuckooFilter::new_random(CuckooConfiguration::builder(1000).build().unwrap());

        filter.insert("basic");

        assert!(filter.contains("basic"));
    }

    #[test]
    fn basic_removal() {
        let filter = CuckooFilter::new_random(CuckooConfiguration::builder(1000).build().unwrap());

        filter.insert("basic");

        assert!(filter.contains("basic"));

        filter.remove("basic");

        assert!(!filter.contains("basic"));
    }

    // TODO: Replace with fake hasher and hashes for more control
    #[test]
    fn lru_insertion() {
        let filter = CuckooFilter::new_random(
            CuckooConfiguration::builder(1000)
                .with_lru(LruConfig {
                    counter_bits: 8.try_into().unwrap(),
                })
                .build()
                .unwrap(),
        );

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
