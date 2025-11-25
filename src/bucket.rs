use std::borrow::BorrowMut;

use crate::{
    associated_data::AssociatedData,
    config::{CuckooConfiguration, LruConfig, TtlConfig},
    data_block::{DataBlock, DataBlockFieldConfiguration, Fingerprint},
};

pub(crate) struct Bucket {
    data: Vec<u8>,
}

impl Bucket {
    // panics on OOM errors
    pub(crate) fn new(configuration: &CuckooConfiguration) -> Self {
        Self {
            data: vec![0; configuration.bucket_byte_size],
        }
    }

    pub(crate) fn insert(
        &mut self,
        fingerprint: &Fingerprint,
        configuration: &CuckooConfiguration,
    ) -> bool {
        for i in 0..configuration.bucket_size {
            let mut data = self.get_data_block(i, configuration);
            let stored = data.get_fingerprint(configuration);

            let reinsert = stored == *fingerprint;

            if !reinsert {
                if stored.is_empty() {
                    data.store_fingerprint(fingerprint, configuration);
                } else {
                    continue;
                }
            }

            if let Some(ttl_config) = configuration.ttl_field_config.as_ref() {
                data.set_ttl(ttl_config, ttl_config.0.ttl.into());
            }
            if let Some(counter_config) = configuration.counter_field_config.as_ref() {
                data.inc_counter(counter_config, 1);
            }
            if let Some(lru_config) = configuration.lru_field_config.as_ref() {
                data.inc_lru_counter(lru_config);
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

    pub(crate) fn contains(
        &mut self,
        fingerprint: &Fingerprint,
        configuration: &CuckooConfiguration,
    ) -> bool {
        for i in 0..configuration.bucket_size {
            let mut data = self.get_data_block(i, configuration);
            let stored = data.get_fingerprint(configuration);

            if stored == *fingerprint {
                if let Some(counter_config) = configuration.counter_field_config.as_ref() {
                    data.inc_counter(counter_config, 1);
                }
                if let Some(lru_config) = configuration.lru_field_config.as_ref() {
                    data.inc_lru_counter(lru_config);
                }
                return true;
            }
        }
        false
    }

    // NOTE: This doesn't update counters and LRU
    pub(crate) fn get_associated_data(
        &mut self,
        fingerprint: &Fingerprint,
        configuration: &CuckooConfiguration,
    ) -> Option<AssociatedData> {
        for i in 0..configuration.bucket_size {
            let data = self.get_data_block(i, configuration);
            let stored = data.get_fingerprint(configuration);

            if stored == *fingerprint {
                return Some(AssociatedData::new(
                    self.get_data_block(i, configuration),
                    configuration.clone(),
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

    pub(crate) fn age_ttl_counters(
        &mut self,
        configuration: &CuckooConfiguration,
        ttl_config: &(TtlConfig, DataBlockFieldConfiguration),
    ) {
        for i in 0..configuration.bucket_size {
            self.get_data_block(i, configuration)
                .age_ttl_counter(ttl_config);
        }
    }
}
