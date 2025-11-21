use std::{
    hash::{BuildHasher, Hash, RandomState},
    iter::repeat_with,
    sync::{Arc, Mutex},
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

// TODO: Update all `mutex posioned` expects
// We should still use expect, because we shouldn't panic anywhere
// But make it clearer that that is a bug in the library
#[allow(clippy::expect_used)]
impl<H: BuildHasher> CuckooFilter<H> {
    pub fn new(configuration: CuckooConfiguration, build_hasher: H) -> Self {
        Self {
            configuration: configuration.clone(),
            buckets: repeat_with(|| Bucket::new(&configuration).into())
                .take(configuration.bucket_count)
                .collect::<Vec<_>>()
                .into(),
            build_hasher,
        }
    }

    pub fn get_bucket_count(&self) -> usize {
        self.configuration.bucket_count
    }

    pub fn insert<K: Hash + ?Sized>(&self, key: &K) -> Option<Fingerprint> {
        let (fp, i1) = self.get_fingerprint_and_index(key);

        let inserted = self.buckets[i1 as usize]
            .lock()
            .expect("mutex poisoned")
            .insert(&fp, &self.configuration);

        if inserted {
            return None;
        }

        let i2 = self.alt_index(&fp, i1);

        let inserted = self.buckets[i2 as usize]
            .lock()
            .expect("mutex poisoned")
            .insert(&fp, &self.configuration);

        if inserted {
            return None;
        }

        let mut cur_index = i1;
        let mut data = vec![0u8; self.configuration.data_block_size];
        let mut cur_data_block = DataBlock::from(&mut data[..]);
        cur_data_block.store_fingerprint(&fp, &self.configuration);
        if let Some(ttl_config) = &self.configuration.ttl_field_config {
            cur_data_block.set_ttl(ttl_config, ttl_config.0.ttl.into());
        }
        if let Some(lru_config) = &self.configuration.lru_field_config {
            cur_data_block.inc_lru_counter(lru_config);
        }
        for _ in 0..self.configuration.max_kicks {
            {
                let mut bucket = self.buckets[cur_index as usize]
                    .lock()
                    .expect("mutex poisoned");
                // Replace a random item first
                if let Some(lru_config) = self.configuration.lru_field_config.as_ref() {
                    if !bucket.kick_lru(&mut cur_data_block, &self.configuration, lru_config) {
                        return Some(cur_data_block.get_fingerprint(&self.configuration));
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
                )
            {
                // Found an alternative spot for evicted item, done with kicks
                return None;
            }
        }

        // Filter is full
        Some(cur_data_block.get_fingerprint(&self.configuration))
    }

    pub fn contains<K: Hash + ?Sized>(&self, key: &K) -> bool {
        let (fp, i1) = self.get_fingerprint_and_index(key);

        let mut contains = self.buckets[i1 as usize]
            .lock()
            .expect("mutex poisoned")
            .contains(&fp, &self.configuration);

        if !contains {
            let i2 = self.alt_index(&fp, i1);
            contains = self.buckets[i2 as usize]
                .lock()
                .expect("mutex poisoned")
                .contains(&fp, &self.configuration);
        }

        contains
    }

    pub fn get_associated_data<K: Hash + ?Sized>(&self, key: &K) -> Option<AssociatedData> {
        let (fp, i1) = self.get_fingerprint_and_index(key);

        let mut contains = self.buckets[i1 as usize]
            .lock()
            .expect("mutex poisoned")
            .get_associated_data(&fp, &self.configuration);

        if contains.is_none() {
            let i2 = self.alt_index(&fp, i1);
            contains = self.buckets[i2 as usize]
                .lock()
                .expect("mutex poisoned")
                .get_associated_data(&fp, &self.configuration);
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

    pub fn full_scan_and_update(&self) {
        for b in self.buckets.iter() {
            let mut bucket = b.lock().expect("mutex poisoned");
            if let Some(lru_config) = &self.configuration.lru_field_config {
                bucket.age_lru_counters(&self.configuration, lru_config);
            }
            if let Some(ttl_config) = &self.configuration.ttl_field_config {
                bucket.age_ttl_counters(&self.configuration, ttl_config);
            }
        }
    }

    pub(crate) fn get_fingerprint<K: Hash + ?Sized>(&self, key: &K) -> Fingerprint {
        self.get_fingerprint_and_index(key).0
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
    use std::{collections::HashSet, hash::Hasher, ops::Range};

    use crate::config::{LruConfig, TtlConfig};

    use super::*;

    fn get_words(range: Range<usize>) -> Vec<String> {
        std::fs::read_to_string("/usr/share/dict/words")
            .unwrap()
            .split("\n")
            .skip(range.start)
            .take(range.len())
            .map(ToString::to_string)
            .collect()
    }

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

    struct PredefinedBucketItem(u64);
    struct TestHasher(u64);
    impl BuildHasher for TestHasher {
        type Hasher = TestHasher;

        fn build_hasher(&self) -> Self::Hasher {
            TestHasher(0)
        }
    }
    impl Hasher for TestHasher {
        fn finish(&self) -> u64 {
            self.0
        }

        fn write(&mut self, bytes: &[u8]) {
            if bytes.len() == 8 {
                self.0 = u64::from_ne_bytes(bytes.try_into().unwrap());
            } else {
                // Shift fingeprint hashes a bit, to allow control
                self.0 = 1 - (u32::from_ne_bytes(bytes.try_into().unwrap()) as u64 % 2);
            }
        }
    }
    impl Hash for PredefinedBucketItem {
        fn hash<H: Hasher>(&self, state: &mut H) {
            state.write_u64(self.0);
        }
    }

    #[test]
    fn lru_insertion() {
        let filter = CuckooFilter::new(
            CuckooConfiguration::builder(1000)
                .bucket_size(2)
                .with_lru(LruConfig {
                    counter_bits: 8.try_into().unwrap(),
                })
                .build()
                .unwrap(),
            TestHasher(0),
        );

        let test_item = PredefinedBucketItem(2 << 32);
        filter.insert(&test_item);
        filter.contains(&test_item); // Make it more used than others

        let test_item_2 = PredefinedBucketItem(4 << 32);
        filter.insert(&test_item_2); // Sharing the same bucket as "test", but less used

        let test_item_3 = PredefinedBucketItem((3 << 32) + 2);
        filter.insert(&test_item_3); // Another bucket, but also valid for "test" bucket
        filter.contains(&test_item_3); // Make it more used

        let test_item_4 = PredefinedBucketItem((5 << 32) + 2);
        filter.insert(&test_item_4); // Takes bucket of "test8", but less used

        // Everything fits now
        assert!(filter.contains(&test_item));
        assert!(filter.contains(&test_item_2));
        assert!(filter.contains(&test_item_3));
        assert!(filter.contains(&test_item_4));

        let test_item_5 = PredefinedBucketItem((1 << 32) + 2);
        // Insert a new item which has to take one of the 2 fully occupied buckets
        filter.insert(&test_item_5);

        assert!(filter.contains(&test_item_2));
        assert!(filter.contains(&test_item));
        assert!(filter.contains(&test_item_3));

        assert!(
            !filter.contains(&test_item_5) || !filter.contains(&test_item_4),
            "No inserted items are missing, but filter can't hold them all"
        );

        // Insert both of these items again and confirm the more used ones are still there
        filter.insert(&test_item_5);
        filter.insert(&test_item_4);
        assert!(filter.contains(&test_item));
        assert!(filter.contains(&test_item_3));
    }

    #[test]
    fn alt_index() {
        let words = get_words(0..200_000);
        let filter = CuckooFilter::new_random(
            CuckooConfiguration::builder(200_000)
                .fingerprint_bits(32.try_into().unwrap())
                .build()
                .unwrap(),
        );

        for word in words {
            let (fp, index) = filter.get_fingerprint_and_index(&word);
            let alt_index = filter.alt_index(&fp, index);
            assert_eq!(index, filter.alt_index(&fp, alt_index));
        }
    }

    // TODO: test kicks and how they preserve/insert new data

    #[test]
    fn full_scan_and_update() {
        let words = get_words(0..100_000);
        let filter = CuckooFilter::new_random(
            CuckooConfiguration::builder(100_000)
                .fingerprint_bits(32.try_into().unwrap())
                .with_lru(LruConfig::default())
                .with_ttl(TtlConfig {
                    ttl: 3.try_into().unwrap(),
                    ttl_bits: 2.try_into().unwrap(),
                })
                .build()
                .unwrap(),
        );

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

        for _ in 0..2 {
            filter.full_scan_and_update();
        }
        for word in stored_words {
            assert!(
                filter.contains(word),
                "Word: {word} expected in the filter, but not found"
            );
        }

        // TTL should remove all entries now
        filter.full_scan_and_update();
        for word in &words {
            assert!(
                !filter.contains(word),
                "Filter contained {word}, but shouldn't have"
            );
        }
    }
}
