use std::borrow::Borrow;

use crate::{config::CuckooConfiguration, data_block::DataBlock};

pub struct AssociatedData {
    data: Box<[u8]>,
    configuration: CuckooConfiguration,
}

impl AssociatedData {
    pub(crate) fn new<T: Borrow<[u8]>>(
        data: DataBlock<T>,
        configuration: CuckooConfiguration,
    ) -> Self {
        Self {
            data: data.inner().into(),
            configuration,
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
}
