use std::time::{Duration, Instant};

use crate::{
    associated_data::AssociatedData,
    data_block::{DataBlock, Fingerprint},
    filter::{CuckooConfiguration, DerivedConfiguration},
};

pub(crate) struct Bucket {
    data: Vec<u8>,
    pub(crate) ttl_baseline: Instant,
}

impl Bucket {
    pub(crate) fn new(
        configuration: &CuckooConfiguration,
        derived: &DerivedConfiguration,
        now: Instant,
    ) -> Self {
        Self {
            data: vec![0; configuration.bucket_size * derived.data_block_size],
            ttl_baseline: now,
        }
    }

    pub(crate) fn insert(
        &mut self,
        fingerprint: &Fingerprint,
        configuration: &CuckooConfiguration,
        derived: &DerivedConfiguration,
        now: Instant,
    ) -> bool {
        let baseline = self.ttl_baseline;
        for i in 0..configuration.bucket_size {
            let mut data = self.get_data_block(i, derived);
            let stored = data.get_fingerprint(derived);

            let mut current_ttl = None;
            if configuration.ttl_enabled {
                current_ttl = Some(data.get_ttl(derived));
            }
            let reinsert = stored == *fingerprint;

            if !reinsert {
                if stored.is_empty() {
                    data.store_fingerprint(fingerprint, derived);
                } else if current_ttl.is_some_and(|t| {
                    baseline + (Duration::from_secs(t.into()) * configuration.ttl_resolution as u32)
                        <= now
                }) {
                    // Clear out whatever TTL or other options it had
                    data.reset();
                    data.store_fingerprint(fingerprint, derived);
                } else {
                    continue;
                }
            }

            let mut ttl_to_store = 0;

            if configuration.ttl_enabled {
                ttl_to_store = (now - baseline).as_secs() as u32
                    / (configuration.ttl_resolution as u32)
                    + configuration.ttl;
                // TTL is too big to store, move up the baseline and readjust
                if ttl_to_store as u64 > 2u64.pow(configuration.ttl_bits as u32) {
                    let diff = now - baseline;
                    self.ttl_baseline += diff;
                    ttl_to_store -= diff.as_secs() as u32 / configuration.ttl_resolution as u32;

                    for i in 0..configuration.bucket_size {
                        let item_ttl = self.get_data_block(i, derived).get_ttl(derived);
                        let item_ttl = item_ttl.saturating_sub(
                            diff.as_secs() as u32 / configuration.ttl_resolution as u32,
                        );
                        self.get_data_block(i, derived).set_ttl(derived, item_ttl);
                    }
                }
            }

            // Fetch data again for borrow checker
            let mut data = self.get_data_block(i, derived);
            if configuration.ttl_enabled {
                data.set_ttl(derived, ttl_to_store);
            }
            if configuration.counter_enabled {
                data.inc_counter(derived, 1);
            }
            if reinsert && configuration.lru_enabled {
                self.increment_lru_counter(i, configuration, derived);
            }
            return true;
        }
        false
    }

    pub(crate) fn kick_random(
        &mut self,
        data_block: &mut DataBlock<'_>,
        configuration: &CuckooConfiguration,
        derived: &DerivedConfiguration,
    ) {
        let index = rand::random_range(0..configuration.bucket_size);
        self.get_data_block(index, derived).swap(data_block);
    }

    pub(crate) fn kick_lru(
        &mut self,
        data_block: &mut DataBlock<'_>,
        configuration: &CuckooConfiguration,
        derived: &DerivedConfiguration,
    ) -> bool {
        let mut min = u8::MAX;
        let mut pos = configuration.bucket_size;
        for i in 0..configuration.bucket_size {
            let data = self.get_data_block(i, derived);
            let counter = data.get_lru_counter(derived);
            if counter < min {
                min = counter;
                pos = i;
            }
        }

        if pos < configuration.bucket_size {
            self.get_data_block(pos, derived).swap(data_block);
            true
        } else {
            false
        }
    }

    pub(crate) fn contains(
        &mut self,
        fingerprint: &Fingerprint,
        configuration: &CuckooConfiguration,
        derived: &DerivedConfiguration,
        now: Instant,
    ) -> bool {
        let baseline = self.ttl_baseline;
        for i in 0..configuration.bucket_size {
            let mut data = self.get_data_block(i, derived);
            let stored = data.get_fingerprint(derived);

            if stored == *fingerprint {
                if configuration.ttl_enabled {
                    let ttl = data.get_ttl(derived);
                    if baseline
                        + Duration::from_secs(ttl as u64) * configuration.ttl_resolution as u32
                        <= now
                    {
                        // Expired item
                        data.reset();
                        return false;
                    }
                }
                if configuration.counter_enabled {
                    data.inc_counter(derived, 1);
                }
                if configuration.lru_enabled {
                    self.increment_lru_counter(i, configuration, derived);
                }
                return true;
            }
        }
        false
    }

    pub(crate) fn get_associated_data(
        &mut self,
        fingerprint: &Fingerprint,
        configuration: &CuckooConfiguration,
        derived: &DerivedConfiguration,
        now: Instant,
    ) -> Option<AssociatedData> {
        let baseline = self.ttl_baseline;
        for i in 0..configuration.bucket_size {
            let mut data = self.get_data_block(i, derived);
            let stored = data.get_fingerprint(derived);

            if stored == *fingerprint {
                if configuration.ttl_enabled {
                    let ttl = data.get_ttl(derived);
                    if baseline
                        + Duration::from_secs(ttl as u64) * configuration.ttl_resolution as u32
                        <= now
                    {
                        // Expired item
                        data.reset();
                        return None;
                    }
                }
                if configuration.counter_enabled {
                    data.inc_counter(derived, 1);
                }
                if configuration.lru_enabled {
                    self.increment_lru_counter(i, configuration, derived);
                }
                return Some(AssociatedData::new(
                    self.get_data_block(i, derived),
                    derived.clone(),
                ));
            }
        }
        None
    }

    pub(crate) fn remove(
        &mut self,
        fingerprint: &Fingerprint,
        configuration: &CuckooConfiguration,
        derived: &DerivedConfiguration,
    ) -> bool {
        for i in 0..configuration.bucket_size {
            let mut data = self.get_data_block(i, derived);
            let stored = data.get_fingerprint(derived);

            if stored == *fingerprint {
                data.reset();
                return true;
            }
        }
        false
    }

    pub(crate) fn get_data_block(
        &mut self,
        index: usize,
        derived: &DerivedConfiguration,
    ) -> DataBlock<'_> {
        let size = derived.data_block_size;
        (&mut self.data[(index * size)..((index + 1) * size)]).into()
    }

    fn increment_lru_counter(
        &mut self,
        index: usize,
        configuration: &CuckooConfiguration,
        derived: &DerivedConfiguration,
    ) {
        if self.get_data_block(index, derived).get_lru_counter(derived) == u8::MAX {
            // Age all counters when one saturates
            for i in 0..configuration.bucket_size {
                self.get_data_block(i, derived).age_lru_counter(derived);
            }
        }
        self.get_data_block(index, derived).inc_lru_counter(derived);
    }
}
