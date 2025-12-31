use std::borrow::{Borrow, BorrowMut};

use crate::{
    associated_data::AssociatedData,
    config::{CuckooConfiguration, LruConfig, TtlConfig},
    data_block::{DataBlock, DataBlockFieldConfiguration, Fingerprint},
};

/// A single bucket in the filter, holding all the fingerprints and their associated data
/// Everything is stored as a single [`Vec<u8>`], where each fingerprint together with its
/// associated data is aligned to [`u8`].
pub(crate) struct Bucket {
    data: Vec<u8>,
}

impl Bucket {
    /// Creates a new bucket based on [`CuckooConfiguration`].
    ///
    /// ## Panics
    ///
    /// Panics on OOM errors (if the requested bucket byte size is too high).
    pub(crate) fn new(configuration: &CuckooConfiguration) -> Self {
        Self {
            data: vec![0; configuration.bucket_byte_size],
        }
    }

    /// Inserts a new fingerprint into this bucket.
    ///
    /// Sets TTL to the default value (if enabled), increments both LRU and generic counters by 1
    /// (if enabled). This is done when the same fingerprint is inserted again, restarting TTL and
    /// increasing counters.
    ///
    /// Returns false if the insertion has failed (if the bucket is fully occupied). In that case,
    /// alternate bucket should be tried and if that fails too, kicking process should be started.
    pub(crate) fn insert<T: Borrow<[u8]>>(
        &mut self,
        data_block: &DataBlock<T>,
        configuration: &CuckooConfiguration,
    ) -> bool {
        let fingerprint = data_block.get_fingerprint(configuration);
        for i in 0..configuration.bucket_size {
            let mut data = self.get_data_block(i, configuration);
            let stored = data.get_fingerprint(configuration);

            let reinsert = stored == fingerprint;

            if !reinsert {
                if stored.is_empty() {
                    data.copy_from(data_block);
                } else {
                    continue;
                }
            } else {
                data.merge_associated_from(data_block, configuration);
            }
            return true;
        }
        false
    }

    /// Kicks a random item from this bucket, by exchaging that [`DataBlock`] with the one
    /// provided.
    ///
    /// This doesn't return. It always succeeds and the kicked item can be found in the provided
    /// [`DataBlock`].
    pub(crate) fn kick_random<T: BorrowMut<[u8]>>(
        &mut self,
        data_block: &mut DataBlock<T>,
        configuration: &CuckooConfiguration,
    ) {
        let index = rand::random_range(0..configuration.bucket_size);
        self.get_data_block(index, configuration).swap(data_block);
    }

    /// Kicks an item from this bucket, based on LRU - kicks out the lowest LRU counter item from
    /// this bucket, that has lower LRU counter than the new item. If the new item has the lowest
    /// LRU counter, kick fails and false is returned. For completely new items (LRU counter == 0),
    /// insertion is guaranteed.
    ///
    /// Returns true if any item was kicked. Returns false if no item was kicked and the new item
    /// was not moved out of [`DataBlock`].
    pub(crate) fn kick_lru<T: BorrowMut<[u8]>>(
        &mut self,
        data_block: &mut DataBlock<T>,
        configuration: &CuckooConfiguration,
        lru_config: &(LruConfig, DataBlockFieldConfiguration),
    ) -> bool {
        let mut min = data_block.get_lru_counter(lru_config);
        if min == 0 {
            // TODO: What happens if LRU is really at 0?
            min = u32::MAX;
        }
        let mut pos = configuration.bucket_size;
        for i in 0..configuration.bucket_size {
            let data = self.get_data_block(i, configuration);
            let counter = data.get_lru_counter(lru_config);
            if counter < min {
                min = counter;
                pos = i;
            }
        }

        if pos < configuration.bucket_size {
            self.get_data_block(pos, configuration).swap(data_block);
            true
        } else {
            false
        }
    }

    /// Looks for the fingerprint in this bucket.
    ///
    /// If fingerprint is found, its LRU and generic counters are also incremented (if enabled).
    ///
    /// Returns true if the fingeprint is stored in this bucket.
    pub(crate) fn contains(
        &mut self,
        fingerprint: &Fingerprint,
        configuration: &CuckooConfiguration,
        update: &LookupValues,
    ) -> bool {
        for i in 0..configuration.bucket_size {
            let mut data = self.get_data_block(i, configuration);
            let stored = data.get_fingerprint(configuration);

            if stored == *fingerprint {
                if let Some(counter_config) = configuration.counter_field_config.as_ref() {
                    data.update_counter(
                        counter_config,
                        update
                            .counter_diff
                            .unwrap_or(counter_config.0.change_on_lookup),
                    );
                }
                if let Some(ttl_config) = configuration.ttl_field_config.as_ref()
                    && let Some(ttl) = update.ttl
                {
                    data.set_ttl(ttl_config, ttl);
                }
                if let Some(lru_config) = configuration.lru_field_config.as_ref() {
                    data.inc_lru_counter(lru_config);
                }
                return true;
            }
        }
        false
    }

    /// Looks for the fingerprint in this bucket and returns its associated data.
    ///
    /// Returns the data associated with the fingerprint, if found.
    pub(crate) fn get_associated_data(
        &mut self,
        fingerprint: &Fingerprint,
        configuration: &CuckooConfiguration,
        update: &LookupValues,
    ) -> Option<AssociatedData> {
        for i in 0..configuration.bucket_size {
            let mut data = self.get_data_block(i, configuration);
            let stored = data.get_fingerprint(configuration);

            if stored == *fingerprint {
                if let Some(counter_config) = configuration.counter_field_config.as_ref() {
                    data.update_counter(
                        counter_config,
                        update
                            .counter_diff
                            .unwrap_or(counter_config.0.change_on_lookup),
                    );
                }
                if let Some(ttl_config) = configuration.ttl_field_config.as_ref()
                    && let Some(ttl) = update.ttl
                {
                    data.set_ttl(ttl_config, ttl);
                }
                if let Some(lru_config) = configuration.lru_field_config.as_ref() {
                    data.inc_lru_counter(lru_config);
                }
                return Some(AssociatedData::new(
                    self.get_data_block(i, configuration),
                    configuration.clone(),
                ));
            }
        }
        None
    }

    /// Removes the fingerprint from this bucket, by clearing out its slot.
    ///
    /// Returns true if fingerprint was found and removed, false if it was not found.
    pub(crate) fn remove(
        &mut self,
        fingerprint: &Fingerprint,
        configuration: &CuckooConfiguration,
    ) -> bool {
        for i in 0..configuration.bucket_size {
            let mut data = self.get_data_block(i, configuration);
            let stored = data.get_fingerprint(configuration);

            if stored == *fingerprint {
                data.reset();
                return true;
            }
        }
        false
    }

    /// Ages all LRU counters in this bucket.
    pub(crate) fn age_lru_counters(
        &mut self,
        configuration: &CuckooConfiguration,
        lru_config: &(LruConfig, DataBlockFieldConfiguration),
    ) {
        for i in 0..configuration.bucket_size {
            self.get_data_block(i, configuration)
                .age_lru_counter(lru_config);
        }
    }

    /// Ages all TTL counters in this bucket.
    ///
    /// Returns the number of removed items after aging.
    pub(crate) fn age_ttl_counters(
        &mut self,
        configuration: &CuckooConfiguration,
        ttl_config: &(TtlConfig, DataBlockFieldConfiguration),
    ) -> usize {
        let mut removed = 0;
        for i in 0..configuration.bucket_size {
            let db = self.get_data_block(i, configuration);
            if db.occupied(configuration) {
                removed += if self
                    .get_data_block(i, configuration)
                    .age_ttl_counter(ttl_config)
                {
                    1
                } else {
                    0
                }
            }
        }
        removed
    }

    fn get_data_block(
        &mut self,
        index: usize,
        configuration: &CuckooConfiguration,
    ) -> DataBlock<&mut [u8]> {
        let size = configuration.data_block_size;
        (&mut self.data[(index * size)..((index + 1) * size)]).into()
    }
}

/// Values to store with the fingerprint on insertion.
#[derive(Default)]
pub struct InsertValues {
    /// TTL to set for the fingerprint on insertion.
    /// This is ignored if the TTL configuration is not enabled.
    pub ttl: Option<u32>,
    /// Counter to set to the fingerprint on insertion.
    /// This is ignored if the counter configuration is not enabled.
    pub counter: Option<i32>,
}

/// Values to store with the fingerprint on lookups.
#[derive(Default)]
pub struct LookupValues {
    /// TTL to set for the fingerprint on lookup.
    /// This is ignored if the TTL configuration is not enabled.
    pub ttl: Option<u32>,
    /// Counter diff to apply (add/remove) to the fingerprint on lookups.
    /// This is ignored if the counter configuration is not enabled.
    pub counter_diff: Option<i32>,
}
