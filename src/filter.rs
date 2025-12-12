use std::{
    hash::{BuildHasher, Hash, RandomState},
    iter::repeat_with,
    sync::{Arc, Mutex, MutexGuard},
};

use crate::{
    associated_data::AssociatedData,
    bucket::Bucket,
    config::CuckooConfiguration,
    data_block::{DataBlock, Fingerprint},
};

/// Thread-safe cuckoo filter, with support for TTL, LRU and custom counters associated with the
/// stored data.
///
/// Instances of [`CuckooFilter`] can be cloned and used across different threads. To ensure thread
/// safety, locks are used, but locking is done per bucket, meaning that 2 separate threads can
/// freely access different buckets without conflicts. In most cases locks shouldn't block, because
/// optimal cuckoo filter configuration will have a large number of buckets, reducing the change of
/// concurrent access to the same bucket.
///
/// # Examples
///
/// Basic cuckoo filter with default configuration
/// ```
/// use cuckoo_clock::{CuckooFilter, config::CuckooConfiguration};
///
/// let filter = CuckooFilter::new_random(CuckooConfiguration::builder(100_000).build()?);
///
/// // None returned from insertion means no entry was evicted
/// assert!(filter.insert("example_data").is_none());
///
/// // Insertion must have been successful
/// assert!(filter.contains("example_data"));
///
/// // Deletion must have been successful
/// assert!(filter.remove("example_data"));
/// assert!(!filter.contains("example_data"));
///
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
///
/// More complex use-case, with additional options
/// ```
/// use cuckoo_clock::{CuckooFilter, config::{CuckooConfiguration, CounterConfig, TtlConfig}};
///
/// let filter = CuckooFilter::new_random(
///     CuckooConfiguration::builder(10_000_000)
///         .fingerprint_bits(18.try_into()?)
///         .bucket_size(8.try_into()?)
///         .with_counter(CounterConfig {
///             counter_bits: 4.try_into()?,
///             ..Default::default()
///         })
///         .with_ttl(TtlConfig {
///             ttl: 600.try_into()?,
///             ttl_bits: 10.try_into()?
///         })
///         .build()?
/// );
///
/// // In this case, we use `insert_if_not_present` to ensure no duplicates, because we care about
/// // the counter
/// // None returned from insertion means no entry was evicted
/// assert!(filter.insert_if_not_present("example_data").is_none());
/// assert!(filter.insert_if_not_present("example_data").is_none());
///
/// // Insertion must have been successful
/// assert!(filter.contains("example_data"));
///
/// // Counter should be 4 now.
/// // We have accessed this item 3 times, but `get_associated_data` also counts as an access.
/// assert_eq!(filter.get_associated_data("example_data").unwrap().get_counter()?, 4);
///
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
///
#[derive(Clone)]
pub struct CuckooFilter<H: BuildHasher> {
    configuration: CuckooConfiguration,
    buckets: Arc<Vec<Mutex<Bucket>>>,
    build_hasher: H,
}

impl CuckooFilter<RandomState> {
    /// Creates a new instance of [`CuckooFilter`], using [`RandomState`] as its [`BuildHasher`].
    ///
    /// # Panics
    ///
    /// Panics if allocation of buckets fails (if too much memory was requested).
    #[must_use]
    pub fn new_random(configuration: CuckooConfiguration) -> Self {
        Self::new(configuration, RandomState::new())
    }
}

impl<H: BuildHasher> CuckooFilter<H> {
    /// Creates a new instance of [`CuckooFilter`], using provided [`BuildHasher`].
    ///
    /// # Panics
    ///
    /// Panics if allocation of buckets fails (if too much memory was requested).
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

    /// Returns the actual bucket count for this [`CuckooFilter`].
    ///
    /// Bucket count is calculated as first next power of two of capacity / bucket_size.
    /// This means that the actual capacity of the filter is usually bigger than the requested
    /// capacity.
    pub const fn get_bucket_count(&self) -> usize {
        self.configuration.bucket_count
    }

    /// Inserts a new item into the filter, only if the filter doesn't contain it alrady.
    ///
    /// This is slower than [`CuckooFilter::insert`], but it ensures that no duplicates are present
    /// in the filter. That can be useful when [`AssociatedData`] is used, to ensure consistent
    /// results.
    ///
    /// Returns fingerprint of the item that was evicted from the filter, if eviction had to take
    /// place to finalize the insertion. It is possible that the item that was just inserted gets
    /// evicted in random kicking process. That can be confirmed using
    /// [`Fingerprint::matches_key`].
    pub fn insert_if_not_present<K: Hash + ?Sized>(&self, key: &K) -> Option<Fingerprint> {
        let (fp, i1) = self.get_fingerprint_and_index(key);

        let mut contains = self
            .lock_bucket(i1 as usize)
            .contains(&fp, &self.configuration);

        if contains {
            return None;
        }

        let i2 = self.alt_index(&fp, i1);
        contains = self
            .lock_bucket(i2 as usize)
            .contains(&fp, &self.configuration);

        if contains {
            return None;
        }

        let mut cur_data_block = self.new_data_block(&fp);

        let inserted = self
            .lock_bucket(i1 as usize)
            .insert(&cur_data_block, &self.configuration);

        if inserted {
            return None;
        }

        let inserted = self
            .lock_bucket(i2 as usize)
            .insert(&cur_data_block, &self.configuration);

        if inserted {
            return None;
        }

        let mut cur_index = if rand::random::<bool>() { i1 } else { i2 };
        for _ in 0..self.configuration.max_kicks {
            {
                let mut bucket = self.lock_bucket(cur_index as usize);
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

            if self
                .lock_bucket(cur_index as usize)
                .insert(&cur_data_block, &self.configuration)
            {
                // Found an alternative spot for evicted item, done with kicks
                return None;
            }
        }

        // Filter is full
        Some(cur_data_block.get_fingerprint(&self.configuration))
    }

    /// Inserts a new item into the filter.
    ///
    /// If both target buckets for this item are full, random item is kicked out of one of these 2
    /// buckets and moved into its alternate bucket, starting a recursive kicking process, which
    /// stops once an empty slot is found in alternate bucket of a kicked item, or
    /// [`crate::config::CuckooConfigurationBuilder::max_kicks`] is reached.
    ///
    /// Returns fingerprint of the item that was evicted from the filter, if eviction had to take
    /// place to finalize the insertion. It is possible that the item that was just inserted gets
    /// evicted in random kicking process. That can be confirmed using
    /// [`Fingerprint::matches_key`].
    pub fn insert<K: Hash + ?Sized>(&self, key: &K) -> Option<Fingerprint> {
        let (fp, i1) = self.get_fingerprint_and_index(key);
        let mut cur_data_block = self.new_data_block(&fp);

        let inserted = self
            .lock_bucket(i1 as usize)
            .insert(&cur_data_block, &self.configuration);

        if inserted {
            return None;
        }

        let i2 = self.alt_index(&fp, i1);

        let inserted = self
            .lock_bucket(i2 as usize)
            .insert(&cur_data_block, &self.configuration);

        if inserted {
            return None;
        }

        let mut cur_index = i1;
        for _ in 0..self.configuration.max_kicks {
            {
                let mut bucket = self.lock_bucket(cur_index as usize);
                // Replace a random item first
                if let Some(lru_config) = self.configuration.lru_field_config.as_ref() {
                    if !bucket.kick_lru(&mut cur_data_block, &self.configuration, lru_config) {
                        return Some(cur_data_block.get_fingerprint(&self.configuration));
                    }
                } else {
                    // TODO: this can even kick the newest item, which is not ideal
                    bucket.kick_random(&mut cur_data_block, &self.configuration);
                }
                cur_index = self.alt_index(
                    &cur_data_block.get_fingerprint(&self.configuration),
                    cur_index,
                );
            }

            if self
                .lock_bucket(cur_index as usize)
                .insert(&cur_data_block, &self.configuration)
            {
                // Found an alternative spot for evicted item, done with kicks
                return None;
            }
        }

        // Filter is full
        Some(cur_data_block.get_fingerprint(&self.configuration))
    }

    /// Check if this key is stored in the filter.
    ///
    /// Returns true if this key might be present in the filter. If false is returned, then the key
    /// is definitely not present.
    pub fn contains<K: Hash + ?Sized>(&self, key: &K) -> bool {
        let (fp, i1) = self.get_fingerprint_and_index(key);

        let mut contains = self
            .lock_bucket(i1 as usize)
            .contains(&fp, &self.configuration);

        if !contains {
            let i2 = self.alt_index(&fp, i1);
            contains = self
                .lock_bucket(i2 as usize)
                .contains(&fp, &self.configuration);
        }

        contains
    }

    /// Loads associated data of a key stored in the filter.
    ///
    /// Returns None if this filter is not present in the filter. Returns associated data for the
    /// first item with the fingerprint matching this key's fingerprint. Note that it is
    /// recommended to use [`CuckooFilter::insert_if_not_present`] if consistent [`AssociatedData`]
    /// is required.
    pub fn get_associated_data<K: Hash + ?Sized>(&self, key: &K) -> Option<AssociatedData> {
        let (fp, i1) = self.get_fingerprint_and_index(key);

        let mut contains = self
            .lock_bucket(i1 as usize)
            .get_associated_data(&fp, &self.configuration);

        if contains.is_none() {
            let i2 = self.alt_index(&fp, i1);
            contains = self
                .lock_bucket(i2 as usize)
                .get_associated_data(&fp, &self.configuration);
        }

        contains
    }

    /// Removes this key from the filter, if present.
    ///
    /// Returns true if the key was present in the filter.
    pub fn remove<K: Hash + ?Sized>(&self, key: &K) -> bool {
        let (fp, i1) = self.get_fingerprint_and_index(key);

        let mut removed = self
            .lock_bucket(i1 as usize)
            .remove(&fp, &self.configuration);

        if !removed {
            let i2 = self.alt_index(&fp, i1);
            removed = self
                .lock_bucket(i2 as usize)
                .remove(&fp, &self.configuration);
        }

        removed
    }

    /// Scans all buckets of this filter and reduces TTL and LRU counters.
    ///
    /// If LRU and/or TTL features are used, this must be called periodically.
    /// Each call to this function will age all the LRU and TTL counters. The frequency of calls
    /// will affect both LRU and TTL in different ways:
    /// - TTL will get reduced by 1 on each call, meaning that scanning each second indirectly sets
    ///   the unit of TTL field to be seconds.
    /// - LRU will get halved on each call. By scanning more frequently, items will require more
    ///   frequent usage to stay in the filter.
    ///
    /// This is a no-op if both LRU and TTL are disabled.
    ///
    /// # Examples
    ///
    /// ```
    /// use cuckoo_clock::{CuckooFilter, config::{CuckooConfiguration, CounterConfig, TtlConfig}};
    ///
    /// let filter = CuckooFilter::new_random(
    ///     CuckooConfiguration::builder(10_000)
    ///         .with_ttl(TtlConfig {
    ///             ttl: 3.try_into()?,
    ///             ttl_bits: 2.try_into()?
    ///         })
    ///         .build()?
    /// );
    ///
    /// filter.insert("example_data");
    ///
    /// assert!(filter.contains("example_data"));
    ///
    /// filter.scan_and_update_full();
    /// assert!(filter.contains("example_data"));
    ///
    /// filter.scan_and_update_full();
    /// assert!(filter.contains("example_data"));
    ///
    /// // The item will get removed now, due to expired TTL
    /// filter.scan_and_update_full();
    /// assert!(!filter.contains("example_data"));
    ///
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn scan_and_update_full(&self) {
        if self.configuration.lru_field_config.is_none()
            && self.configuration.ttl_field_config.is_none()
        {
            return;
        }

        for b in self.buckets.iter() {
            #[expect(clippy::unwrap_used)]
            let mut bucket = b.lock().unwrap();
            if let Some(lru_config) = &self.configuration.lru_field_config {
                bucket.age_lru_counters(&self.configuration, lru_config);
            }
            if let Some(ttl_config) = &self.configuration.ttl_field_config {
                bucket.age_ttl_counters(&self.configuration, ttl_config);
            }
        }
    }

    /// Scans all buckets of this filter and reduces TTL counters.
    ///
    /// Similar to [`CuckooFilter::scan_and_update_full`], but updates only TTL counters. This
    /// allows more control, enabling different update frequency for TTL and LRU.
    ///
    /// This is a no-op if TTL is disabled.
    pub fn scan_and_update_ttl(&self) {
        if self.configuration.ttl_field_config.is_none() {
            return;
        }

        for b in self.buckets.iter() {
            #[expect(clippy::unwrap_used)]
            let mut bucket = b.lock().unwrap();
            if let Some(ttl_config) = &self.configuration.ttl_field_config {
                bucket.age_ttl_counters(&self.configuration, ttl_config);
            }
        }
    }

    /// Scans all buckets of this filter and reduces LRU counters.
    ///
    /// Similar to [`CuckooFilter::scan_and_update_full`], but updates only LRU counters. This
    /// allows more control, enabling different update frequency for TTL and LRU.
    ///
    /// This is a no-op if LRU is disabled.
    pub fn scan_and_update_lru(&self) {
        if self.configuration.lru_field_config.is_none() {
            return;
        }

        for b in self.buckets.iter() {
            #[expect(clippy::unwrap_used)]
            let mut bucket = b.lock().unwrap();
            if let Some(lru_config) = &self.configuration.lru_field_config {
                bucket.age_lru_counters(&self.configuration, lru_config);
            }
        }
    }

    /// Generates the fingerprint and first index for the provided key.
    pub(crate) fn get_fingerprint<K: Hash + ?Sized>(&self, key: &K) -> Fingerprint {
        self.get_fingerprint_and_index(key).0
    }

    fn new_data_block(&self, fp: &Fingerprint) -> DataBlock<Vec<u8>> {
        let data = vec![0u8; self.configuration.data_block_size];
        let mut cur_data_block = DataBlock::from(data);
        cur_data_block.store_fingerprint(fp, &self.configuration);

        if let Some(ttl_config) = self.configuration.ttl_field_config.as_ref() {
            cur_data_block.set_ttl(ttl_config, ttl_config.0.ttl.into());
        }
        if let Some(counter_config) = self.configuration.counter_field_config.as_ref() {
            cur_data_block.update_counter(counter_config, counter_config.0.change_on_insert);
        }
        if let Some(lru_config) = self.configuration.lru_field_config.as_ref() {
            cur_data_block.inc_lru_counter(lru_config);
        }
        cur_data_block
    }

    fn get_fingerprint_and_index<K: Hash + ?Sized>(&self, key: &K) -> (Fingerprint, u32) {
        let result = self.build_hasher.hash_one(key);

        // Fingeprint bits over 32 are definitely an overkill
        // We can reduce number of hashes by using one hash as fingerprint and first index
        let fingerprint = (result >> 32) as u32;
        // Intentional truncation here
        #[expect(clippy::cast_possible_truncation)]
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
    #[expect(clippy::cast_possible_truncation)]
    fn alt_index(&self, fingerprint: &Fingerprint, index: u32) -> u32 {
        let result = self.build_hasher.hash_one(fingerprint);

        (index ^ ((result as u32) & self.configuration.buckets_mask))
            & self.configuration.buckets_mask
    }

    #[expect(clippy::unwrap_used)]
    fn lock_bucket(&self, index: usize) -> MutexGuard<'_, Bucket> {
        // Any panic while lock is held should come from this library
        // Any panic produced while the lock is held is a bug in the library!
        self.buckets[index].lock().unwrap()
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used)]
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
                .bucket_size(2.try_into().unwrap())
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
        filter.insert(&test_item_4); // Takes bucket of "test_item_3", but less used

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

    #[test]
    fn random_kicks() {
        let filter = CuckooFilter::new(
            CuckooConfiguration::builder(1000)
                .bucket_size(2.try_into().unwrap())
                .build()
                .unwrap(),
            TestHasher(0),
        );

        let test_item = PredefinedBucketItem(2 << 32);
        filter.insert(&test_item);

        let test_item_2 = PredefinedBucketItem(4 << 32);
        filter.insert(&test_item_2); // Sharing the same bucket as "test"

        let test_item_3 = PredefinedBucketItem((3 << 32) + 2);
        filter.insert(&test_item_3); // Another bucket, but also valid for "test" bucket

        let test_item_4 = PredefinedBucketItem((5 << 32) + 2);
        filter.insert(&test_item_4); // Takes bucket of "test_item_3"

        // This one should not be kicked, because it takes an unrelated bucket
        let test_item_unrelated = PredefinedBucketItem((10 << 32) + 10);
        filter.insert(&test_item_unrelated);

        // Everything fits now
        assert!(filter.contains(&test_item));
        assert!(filter.contains(&test_item_2));
        assert!(filter.contains(&test_item_3));
        assert!(filter.contains(&test_item_4));

        let test_item_5 = PredefinedBucketItem((1 << 32) + 2);
        // Insert a new item which has to take one of the 2 fully occupied buckets
        let kicked = filter.insert(&test_item_5);
        assert!(kicked.is_some(), "An item had to be kicked");
        assert!(filter.contains(&test_item_5));
        assert!(filter.contains(&test_item_unrelated));

        for item in [&test_item, &test_item_2, &test_item_3, &test_item_4]
            .iter()
            .filter(|i| !kicked.as_ref().unwrap().matches_key(i, &filter))
        {
            assert!(filter.contains(item), "Only one item should be kicked");
        }
    }

    #[test]
    fn scan_and_update_full() {
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
            filter.scan_and_update_full();
        }
        for word in stored_words {
            assert!(
                filter.contains(word),
                "Word: {word} expected in the filter, but not found"
            );
        }

        // TTL should remove all entries now
        filter.scan_and_update_full();
        for word in &words {
            assert!(
                !filter.contains(word),
                "Filter contained {word}, but shouldn't have"
            );
        }
    }
}
