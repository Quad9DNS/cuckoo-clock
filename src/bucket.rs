use std::time::{Duration, Instant};

use crate::{
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
            data: vec![
                0;
                configuration.bucket_size
                    * (derived.fingerprint_bytes
                        + (if configuration.lru_enabled { 1 } else { 0 })
                        + (if configuration.ttl_enabled {
                            derived.ttl_bytes
                        } else {
                            0
                        })
                        + (if configuration.counter_enabled {
                            derived.counter_bytes
                        } else {
                            0
                        }))
            ],
            ttl_baseline: now,
        }
    }

    pub(crate) fn insert(
        &mut self,
        fingerprint: &Fingerprint,
        configuration: &CuckooConfiguration,
        now: Instant,
    ) -> bool {
        let baseline = self.ttl_baseline;
        for i in 0..configuration.bucket_size {
            let mut data = self.get_data_block(i, configuration);
            let stored = data.get_fingerprint(configuration);

            let mut current_ttl = None;
            if configuration.ttl_enabled {
                current_ttl = Some(data.get_ttl(configuration));
            }
            let reinsert = stored == *fingerprint;

            if !reinsert {
                if stored.is_empty() {
                    data.store_fingerprint(fingerprint, configuration);
                } else if current_ttl.is_some_and(|t| {
                    baseline + (Duration::from_secs(t.into()) * configuration.ttl_resolution as u32)
                        <= now
                }) {
                    // Clear out whatever TTL or other options it had
                    data.reset();
                    data.store_fingerprint(fingerprint, configuration);
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
                        let item_ttl = self.get_data_block(i, configuration).get_ttl(configuration);
                        let item_ttl = item_ttl.saturating_sub(
                            diff.as_secs() as u32 / configuration.ttl_resolution as u32,
                        );
                        self.get_data_block(i, configuration)
                            .set_ttl(configuration, item_ttl);
                    }
                }
            }

            // Fetch data again for borrow checker
            let mut data = self.get_data_block(i, configuration);
            if configuration.ttl_enabled {
                data.set_ttl(configuration, ttl_to_store);
            }
            if configuration.counter_enabled {
                data.inc_counter(configuration, 1);
            }
            if reinsert && configuration.lru_enabled {
                self.increment_lru_counter(i, configuration);
            }
            return true;
        }
        false
    }

    pub(crate) fn kick_random(
        &mut self,
        data_block: &mut DataBlock<'_>,
        configuration: &CuckooConfiguration,
    ) {
        let index = rand::random_range(0..configuration.bucket_size);
        self.get_data_block(index, configuration).swap(data_block);
    }

    pub(crate) fn kick_lru(
        &mut self,
        data_block: &mut DataBlock<'_>,
        configuration: &CuckooConfiguration,
    ) -> bool {
        let mut min = u8::MAX;
        let mut pos = configuration.bucket_size;
        for i in 0..configuration.bucket_size {
            let data = self.get_data_block(i, configuration);
            let counter = data.get_lru_counter(configuration);
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

    pub(crate) fn contains(
        &mut self,
        fingerprint: &Fingerprint,
        configuration: &CuckooConfiguration,
        now: Instant,
    ) -> bool {
        let baseline = self.ttl_baseline;
        for i in 0..configuration.bucket_size {
            let mut data = self.get_data_block(i, configuration);
            let stored = data.get_fingerprint(configuration);

            if stored == *fingerprint {
                if configuration.ttl_enabled {
                    let ttl = data.get_ttl(configuration);
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
                    data.inc_counter(configuration, 1);
                }
                if configuration.lru_enabled {
                    self.increment_lru_counter(i, configuration);
                }
                return true;
            }
        }
        false
    }

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

    pub(crate) fn get_data_block(
        &mut self,
        index: usize,
        configuration: &CuckooConfiguration,
    ) -> DataBlock<'_> {
        let size = DataBlock::<'_>::get_size(configuration);
        (&mut self.data[(index * size)..((index + 1) * size)]).into()
    }

    fn increment_lru_counter(&mut self, index: usize, configuration: &CuckooConfiguration) {
        if self
            .get_data_block(index, configuration)
            .get_lru_counter(configuration)
            == u8::MAX
        {
            // Age all counters when one saturates
            for i in 0..configuration.bucket_size {
                self.get_data_block(i, configuration)
                    .age_lru_counter(configuration);
            }
        }
        self.get_data_block(index, configuration)
            .inc_lru_counter(configuration);
    }
}
