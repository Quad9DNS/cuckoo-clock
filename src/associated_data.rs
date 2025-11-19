use std::{
    borrow::Borrow,
    time::{Duration, Instant},
};

use crate::{config::CuckooConfiguration, data_block::DataBlock};

pub struct AssociatedData {
    data: Box<[u8]>,
    configuration: CuckooConfiguration,
    ttl_baseline: Instant,
}

impl AssociatedData {
    pub(crate) fn new<T: Borrow<[u8]>>(
        data: DataBlock<T>,
        configuration: CuckooConfiguration,
        ttl_baseline: Instant,
    ) -> Self {
        Self {
            data: data.inner().into(),
            configuration,
            ttl_baseline,
        }
    }

    #[must_use]
    pub fn get_fingerprint(&self) -> u32 {
        DataBlock::from(&self.data[..])
            .get_fingerprint(&self.configuration)
            .data()
    }

    pub fn get_lru_counter(&self) -> crate::Result<u8> {
        Ok(DataBlock::from(&self.data[..]).get_lru_counter(
            self.configuration
                .lru_field_config
                .as_ref()
                .ok_or(crate::Error::FeatureNotEnabled("LRU".to_string()))?,
        ))
    }

    pub fn get_counter(&self) -> crate::Result<u32> {
        Ok(DataBlock::from(&self.data[..]).get_counter(
            self.configuration
                .counter_field_config
                .as_ref()
                .ok_or(crate::Error::FeatureNotEnabled("Counter".to_string()))?,
        ))
    }

    pub fn get_stored_ttl_value(&self) -> crate::Result<u32> {
        Ok(DataBlock::from(&self.data[..]).get_ttl(
            self.configuration
                .ttl_field_config
                .as_ref()
                .ok_or(crate::Error::FeatureNotEnabled("TTL".to_string()))?,
        ))
    }

    pub fn get_ttl(&self) -> crate::Result<u64> {
        self.get_ttl_at(Instant::now())
    }

    pub fn get_expiry(&self) -> crate::Result<Instant> {
        let ttl_config = self
            .configuration
            .ttl_field_config
            .as_ref()
            .ok_or(crate::Error::FeatureNotEnabled("TTL".to_string()))?;
        let ttl = self.get_stored_ttl_value()?;
        let expiry = self.ttl_baseline
            + Duration::from_secs(ttl as u64) * ttl_config.0.ttl_resolution.into();
        Ok(expiry)
    }

    pub fn get_ttl_at(&self, at: Instant) -> crate::Result<u64> {
        let expiry = self.get_expiry()?;
        Ok((expiry - at).as_secs())
    }
}
