use std::time::{Duration, Instant};

use crate::{
    data_block::{DataBlock, ReadOnlyDataBlock},
    filter::{CuckooConfiguration, DerivedConfiguration},
};

pub struct AssociatedData {
    data: Box<[u8]>,
    configuration: CuckooConfiguration,
    derived: DerivedConfiguration,
    ttl_baseline: Instant,
}

impl AssociatedData {
    pub(crate) fn new(
        data: DataBlock<'_>,
        configuration: CuckooConfiguration,
        derived: DerivedConfiguration,
        ttl_baseline: Instant,
    ) -> Self {
        Self {
            data: data.inner().into(),
            configuration,
            derived,
            ttl_baseline,
        }
    }

    #[must_use]
    pub fn get_fingerprint(&self) -> u32 {
        ReadOnlyDataBlock::from(&self.data[..])
            .get_fingerprint(&self.derived)
            .data()
    }

    #[must_use]
    pub fn get_lru_counter(&self) -> u8 {
        ReadOnlyDataBlock::from(&self.data[..]).get_lru_counter(&self.derived)
    }

    #[must_use]
    pub fn get_counter(&self) -> u32 {
        ReadOnlyDataBlock::from(&self.data[..]).get_counter(&self.derived)
    }

    #[must_use]
    pub fn get_ttl(&self) -> u64 {
        self.get_ttl_at(Instant::now())
    }

    #[must_use]
    pub fn get_ttl_at(&self, at: Instant) -> u64 {
        let ttl = ReadOnlyDataBlock::from(&self.data[..]).get_ttl(&self.derived);
        let expiry =
            self.ttl_baseline + Duration::from_secs(ttl as u64) * self.configuration.ttl_resolution;
        (expiry - at).as_secs()
    }
}
