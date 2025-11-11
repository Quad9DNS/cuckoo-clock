use std::{
    iter::repeat_n,
    time::{Duration, Instant},
};

use crate::filter::{CuckooConfiguration, DerivedConfiguration};

#[derive(Hash, Clone, PartialEq, Eq)]
pub(crate) struct Fingerprint {
    data: Box<[u8]>,
}

impl Fingerprint {
    pub(crate) fn new(hash: u32, bytes: usize, mask: u32) -> Self {
        let mut fingerprint = hash & mask;
        if fingerprint == 0 {
            fingerprint = 1;
        }

        Self {
            data: fingerprint.to_ne_bytes()[0..bytes]
                .to_vec()
                .into_boxed_slice(),
        }
    }

    pub(crate) fn new_empty(length: usize) -> Self {
        Self {
            data: vec![0; length].into_boxed_slice(),
        }
    }

    pub(crate) fn data(&self) -> &[u8] {
        &self.data
    }

    fn is_empty(&self) -> bool {
        self.data.iter().all(|b| *b == 0)
    }
}

impl From<Box<[u8]>> for Fingerprint {
    fn from(value: Box<[u8]>) -> Self {
        Self { data: value }
    }
}

impl From<&[u8]> for Fingerprint {
    fn from(value: &[u8]) -> Self {
        value.to_vec().into_boxed_slice().into()
    }
}

impl From<Fingerprint> for Box<[u8]> {
    fn from(value: Fingerprint) -> Self {
        value.data
    }
}

pub(crate) struct Bucket {
    data: Vec<u8>,
    pub(crate) ttl_baseline: Instant,
}

pub(crate) struct DataBlock<'a>(&'a mut [u8]);

impl<'a> From<&'a mut [u8]> for DataBlock<'a> {
    fn from(value: &'a mut [u8]) -> Self {
        Self(value)
    }
}

impl<'a> DataBlock<'a> {
    pub(crate) fn get_size(
        configuration: &CuckooConfiguration,
        derived: &DerivedConfiguration,
    ) -> usize {
        let mut size = derived.fingerprint_bytes;
        if configuration.lru_enabled {
            size += 1;
        }
        if configuration.ttl_enabled {
            size += derived.ttl_bytes
        }
        size
    }

    pub(crate) fn get_fingerprint(&self, derived: &DerivedConfiguration) -> Fingerprint {
        self.0[0..derived.fingerprint_bytes].into()
    }

    pub(crate) fn store_fingerprint(
        &mut self,
        fingerprint: &Fingerprint,
        derived: &DerivedConfiguration,
    ) {
        self.0[0..derived.fingerprint_bytes].copy_from_slice(&fingerprint.data);
    }

    pub(crate) fn reset(
        &mut self,
        configuration: &CuckooConfiguration,
        derived: &DerivedConfiguration,
    ) {
        self.0[0..derived.fingerprint_bytes]
            .copy_from_slice(&Fingerprint::new_empty(derived.fingerprint_bytes).data);
        let mut rest_start = derived.fingerprint_bytes;
        if configuration.lru_enabled {
            self.0[rest_start] = 0;
            rest_start += 1;
        }
        if configuration.ttl_enabled {
            self.0[rest_start..rest_start + derived.ttl_bytes].copy_from_slice(&vec![
                0;
                derived
                    .ttl_bytes
            ]);
        }
    }

    pub(crate) fn swap(
        &mut self,
        other: &mut DataBlock<'_>,
        configuration: &CuckooConfiguration,
        derived: &DerivedConfiguration,
    ) {
        assert_eq!(
            self.0.len(),
            other.0.len(),
            "Two Cuckoo data blocks should have equal sizes"
        );
        assert_ne!(
            self.0.as_ptr(),
            other.0.as_ptr(),
            "Tried to swap the same 2 data blocks"
        );
        unsafe {
            std::ptr::swap_nonoverlapping(
                self.0.as_mut_ptr(),
                other.0.as_mut_ptr(),
                Self::get_size(configuration, derived),
            );
        }
    }

    pub(crate) fn get_lru_counter(&self, derived: &DerivedConfiguration) -> u8 {
        self.0[derived.fingerprint_bytes]
    }

    pub(crate) fn get_lru_counter_mut(&mut self, derived: &DerivedConfiguration) -> &mut u8 {
        &mut self.0[derived.fingerprint_bytes]
    }

    pub(crate) fn get_ttl(
        &self,
        configuration: &CuckooConfiguration,
        derived: &DerivedConfiguration,
    ) -> u32 {
        let mut ttl_start = derived.fingerprint_bytes;
        if configuration.lru_enabled {
            ttl_start += 1;
        }
        let ttl_bytes = &self.0[ttl_start..ttl_start + derived.ttl_bytes];
        let padding = repeat_n(0u8, 4 - ttl_bytes.len())
            .chain(ttl_bytes.iter().copied())
            .take(4)
            .collect::<Vec<_>>();
        u32::from_be_bytes(
            padding
                .try_into()
                .expect("TTL was not properly padded to 4 bytes!"),
        )
    }

    pub(crate) fn set_ttl(
        &mut self,
        configuration: &CuckooConfiguration,
        derived: &DerivedConfiguration,
        ttl: u32,
    ) {
        let mut ttl_start = derived.fingerprint_bytes;
        if configuration.lru_enabled {
            ttl_start += 1;
        }
        self.0[ttl_start..ttl_start + derived.ttl_bytes]
            .copy_from_slice(&ttl.to_be_bytes()[0..derived.ttl_bytes]);
    }
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
                        }))
            ],
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
            let mut data = self.get_data_block(i, configuration, derived);
            let stored = data.get_fingerprint(derived);

            let mut current_ttl = None;
            if configuration.ttl_enabled {
                current_ttl = Some(data.get_ttl(configuration, derived));
            }
            let reinsert = stored == *fingerprint;

            if !reinsert && stored.is_empty()
                || (current_ttl.is_some_and(|t| {
                    baseline + (Duration::from_secs(t.into()) * configuration.ttl_resolution as u32)
                        <= now
                }))
            {
                data.store_fingerprint(fingerprint, derived);
            } else if !reinsert {
                continue;
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
                        let item_ttl = self
                            .get_data_block(i, configuration, derived)
                            .get_ttl(configuration, derived);
                        let item_ttl = item_ttl.saturating_sub(
                            diff.as_secs() as u32 / configuration.ttl_resolution as u32,
                        );
                        self.get_data_block(i, configuration, derived).set_ttl(
                            configuration,
                            derived,
                            item_ttl,
                        );
                    }
                }
            }

            // Fetch data again for borrow checker
            let mut data = self.get_data_block(i, configuration, derived);
            if configuration.ttl_enabled {
                data.set_ttl(configuration, derived, ttl_to_store);
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
        self.get_data_block(index, configuration, derived)
            .swap(data_block, configuration, derived);
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
            let data = self.get_data_block(i, configuration, derived);
            let counter = data.get_lru_counter(derived);
            if counter < min {
                min = counter;
                pos = i;
            }
        }

        if pos < configuration.bucket_size {
            self.get_data_block(pos, configuration, derived).swap(
                data_block,
                configuration,
                derived,
            );
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
            let mut data = self.get_data_block(i, configuration, derived);
            let stored = data.get_fingerprint(derived);

            if stored == *fingerprint {
                if configuration.ttl_enabled {
                    let ttl = data.get_ttl(configuration, derived);
                    if baseline
                        + Duration::from_secs(ttl as u64) * configuration.ttl_resolution as u32
                        <= now
                    {
                        // Expired item
                        data.reset(configuration, derived);
                        return false;
                    }
                }
                if configuration.lru_enabled {
                    self.increment_lru_counter(i, configuration, derived);
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
        derived: &DerivedConfiguration,
    ) -> bool {
        for i in 0..configuration.bucket_size {
            let mut data = self.get_data_block(i, configuration, derived);
            let stored = data.get_fingerprint(derived);

            if stored == *fingerprint {
                data.reset(configuration, derived);
                return true;
            }
        }
        false
    }

    pub(crate) fn get_data_block(
        &mut self,
        index: usize,
        configuration: &CuckooConfiguration,
        derived: &DerivedConfiguration,
    ) -> DataBlock<'_> {
        let size = DataBlock::<'_>::get_size(configuration, derived);
        (&mut self.data[(index * size)..((index + 1) * size)]).into()
    }

    fn increment_lru_counter(
        &mut self,
        index: usize,
        configuration: &CuckooConfiguration,
        derived: &DerivedConfiguration,
    ) {
        if self
            .get_data_block(index, configuration, derived)
            .get_lru_counter(derived)
            == u8::MAX
        {
            // Age all counters when one saturates
            for i in 0..configuration.bucket_size {
                *self
                    .get_data_block(i, configuration, derived)
                    .get_lru_counter_mut(derived) >>= 1;
            }
        }
        *self
            .get_data_block(index, configuration, derived)
            .get_lru_counter_mut(derived) += 1;
    }
}
