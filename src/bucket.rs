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
        if configuration.lru_enabled {
            derived.fingerprint_bytes + 1
        } else {
            derived.fingerprint_bytes
        }
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
        if configuration.lru_enabled {
            self.0[derived.fingerprint_bytes] = 0;
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
}

impl Bucket {
    pub(crate) fn new(configuration: &CuckooConfiguration, derived: &DerivedConfiguration) -> Self {
        Self {
            data: vec![
                0;
                configuration.bucket_size
                    * (derived.fingerprint_bytes
                        + (if configuration.lru_enabled { 1 } else { 0 }))
            ],
        }
    }

    pub(crate) fn insert(
        &mut self,
        fingerprint: &Fingerprint,
        configuration: &CuckooConfiguration,
        derived: &DerivedConfiguration,
    ) -> bool {
        for i in 0..configuration.bucket_size {
            let mut data = self.get_data_block(i, configuration, derived);
            let stored = data.get_fingerprint(derived);

            if stored == *fingerprint {
                if configuration.lru_enabled {
                    self.increment_lru_counter(i, configuration, derived);
                }
                return true;
            } else if stored.is_empty() {
                data.store_fingerprint(fingerprint, derived);
                return true;
            }
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
    ) -> bool {
        for i in 0..configuration.bucket_size {
            let data = self.get_data_block(i, configuration, derived);
            let stored = data.get_fingerprint(derived);

            if stored == *fingerprint {
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
