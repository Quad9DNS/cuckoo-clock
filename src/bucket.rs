use std::{
    borrow::BorrowMut,
    time::{Duration, Instant},
};

use crate::{
    associated_data::AssociatedData,
    config::{CuckooConfiguration, LruConfig},
    data_block::{DataBlock, DataBlockFieldConfiguration, Fingerprint},
};

pub(crate) struct Bucket {
    data: Vec<u8>,
    pub(crate) ttl_baseline: Instant,
}

impl Bucket {
    pub(crate) fn new(configuration: &CuckooConfiguration, now: Instant) -> Self {
        Self {
            data: vec![0; configuration.bucket_byte_size],
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

            let reinsert = stored == *fingerprint;

            if !reinsert {
                if stored.is_empty() {
                    data.store_fingerprint(fingerprint, configuration);
                } else if configuration.ttl_field_config.as_ref().is_some_and(|t| {
                    baseline
                        + (Duration::from_secs(data.get_ttl(t).into()) * t.0.ttl_resolution.into())
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

            if let Some(ttl_config) = &configuration.ttl_field_config {
                let ttl_from_baseline = (now - baseline).as_secs().try_into().map(|d: u32| {
                    d / u32::from(ttl_config.0.ttl_resolution) + u32::from(ttl_config.0.ttl)
                });
                // TTL is too big to store, move up the baseline and readjust
                // diff / resolution won't be > 32 bits inside this block
                #[allow(clippy::cast_possible_truncation)]
                if ttl_from_baseline.is_err()
                    || ttl_from_baseline
                        .is_ok_and(|ttl| ttl as u64 > 2u64.pow(ttl_config.0.ttl_bits.into()))
                {
                    let diff = now - baseline;
                    self.ttl_baseline += diff;
                    ttl_to_store -=
                        (diff.as_secs() / u32::from(ttl_config.0.ttl_resolution) as u64) as u32;

                    for i in 0..configuration.bucket_size {
                        let item_ttl = self.get_data_block(i, configuration).get_ttl(ttl_config);
                        let item_ttl = item_ttl.saturating_sub(
                            (diff.as_secs() / u32::from(ttl_config.0.ttl_resolution) as u64) as u32,
                        );
                        self.get_data_block(i, configuration)
                            .set_ttl(ttl_config, item_ttl);
                    }
                }
            }

            // Fetch data again for borrow checker
            let mut data = self.get_data_block(i, configuration);
            if let Some(ttl_config) = configuration.ttl_field_config.as_ref() {
                data.set_ttl(ttl_config, ttl_to_store);
            }
            if let Some(counter_config) = configuration.counter_field_config.as_ref() {
                data.inc_counter(counter_config, 1);
            }
            if reinsert && let Some(lru_config) = configuration.lru_field_config.as_ref() {
                self.increment_lru_counter(i, configuration, lru_config);
            }
            return true;
        }
        false
    }

    pub(crate) fn kick_random<T: BorrowMut<[u8]>>(
        &mut self,
        data_block: &mut DataBlock<T>,
        configuration: &CuckooConfiguration,
    ) {
        let index = rand::random_range(0..configuration.bucket_size);
        self.get_data_block(index, configuration).swap(data_block);
    }

    pub(crate) fn kick_lru<T: BorrowMut<[u8]>>(
        &mut self,
        data_block: &mut DataBlock<T>,
        configuration: &CuckooConfiguration,
        lru_config: &(LruConfig, DataBlockFieldConfiguration),
    ) -> bool {
        let mut min = u8::MAX;
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
                if let Some(ttl_config) = &configuration.ttl_field_config {
                    let ttl = data.get_ttl(ttl_config);
                    if baseline
                        + Duration::from_secs(ttl as u64) * ttl_config.0.ttl_resolution.into()
                        <= now
                    {
                        // Expired item
                        data.reset();
                        return false;
                    }
                }
                if let Some(counter_config) = configuration.counter_field_config.as_ref() {
                    data.inc_counter(counter_config, 1);
                }
                if let Some(lru_config) = configuration.lru_field_config.as_ref() {
                    self.increment_lru_counter(i, configuration, lru_config);
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
        now: Instant,
    ) -> Option<AssociatedData> {
        let baseline = self.ttl_baseline;
        for i in 0..configuration.bucket_size {
            let mut data = self.get_data_block(i, configuration);
            let stored = data.get_fingerprint(configuration);

            if stored == *fingerprint {
                if let Some(ttl_config) = &configuration.ttl_field_config {
                    let ttl = data.get_ttl(ttl_config);
                    if baseline
                        + Duration::from_secs(ttl as u64) * u32::from(ttl_config.0.ttl_resolution)
                        <= now
                    {
                        // Expired item
                        data.reset();
                        return None;
                    }
                }
                if let Some(counter_config) = configuration.counter_field_config.as_ref() {
                    data.inc_counter(counter_config, 1);
                }
                if let Some(lru_config) = configuration.lru_field_config.as_ref() {
                    self.increment_lru_counter(i, configuration, lru_config);
                }
                let baseline = self.ttl_baseline;
                return Some(AssociatedData::new(
                    self.get_data_block(i, configuration),
                    configuration.clone(),
                    baseline,
                ));
            }
        }
        None
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
    ) -> DataBlock<&mut [u8]> {
        let size = configuration.data_block_size;
        (&mut self.data[(index * size)..((index + 1) * size)]).into()
    }

    fn increment_lru_counter(
        &mut self,
        index: usize,
        configuration: &CuckooConfiguration,
        lru_config: &(LruConfig, DataBlockFieldConfiguration),
    ) {
        if self
            .get_data_block(index, configuration)
            .get_lru_counter(lru_config)
            == u8::MAX
        {
            // Age all counters when one saturates
            for i in 0..configuration.bucket_size {
                self.get_data_block(i, configuration)
                    .age_lru_counter(lru_config);
            }
        }
        self.get_data_block(index, configuration)
            .inc_lru_counter(lru_config);
    }
}
