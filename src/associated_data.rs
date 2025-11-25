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

    pub fn get_lru_counter(&self) -> crate::Result<u32> {
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

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use crate::{
        Fingerprint,
        config::{LruConfig, TtlConfig},
    };

    use super::*;

    #[test]
    fn basic_fingerprint_access() {
        for bit_count in 1..=32 {
            let config = CuckooConfiguration::builder(1000)
                .fingerprint_bits(bit_count.try_into().unwrap())
                .build()
                .unwrap();

            let mut data = [0u8; 4];
            let mut data_block = DataBlock::from(&mut data[..]);
            data_block.store_fingerprint(&Fingerprint::new(1, 1), &config);

            let associated_data = AssociatedData::new(data_block, config);
            assert_eq!(associated_data.get_fingerprint(), 1);

            // Associated data should be a snapshot
            // Further modifications should not change it
            data[0] = 100;
            assert_eq!(associated_data.get_fingerprint(), 1);
        }
    }

    #[test]
    fn disabled_features() {
        // No additional features
        let config = CuckooConfiguration::builder(1000)
            .fingerprint_bits(8.try_into().unwrap())
            .build()
            .unwrap();

        let data = [0u8; 4];
        let data_block = DataBlock::from(&data[..]);
        let associated_data = AssociatedData::new(data_block, config);

        let Err(crate::Error::FeatureNotEnabled(feature_name)) =
            associated_data.get_stored_ttl_value()
        else {
            panic!("TTL not enabled error should be returned!");
        };
        assert_eq!(feature_name, "TTL");

        let Err(crate::Error::FeatureNotEnabled(feature_name)) = associated_data.get_lru_counter()
        else {
            panic!("LRU not enabled error should be returned!");
        };
        assert_eq!(feature_name, "LRU");

        let Err(crate::Error::FeatureNotEnabled(feature_name)) = associated_data.get_counter()
        else {
            panic!("Counter not enabled error should be returned!");
        };
        assert_eq!(feature_name, "Counter");
    }

    #[test]
    fn ttl_access() {
        for bit_count in 1..=32 {
            // No additional features
            let config = CuckooConfiguration::builder(1000)
                .fingerprint_bits(5.try_into().unwrap())
                .with_ttl(TtlConfig {
                    ttl: 10.try_into().unwrap(),
                    ttl_bits: bit_count.try_into().unwrap(),
                })
                .build()
                .unwrap();

            let mut data = [0u8; 5];
            let mut data_block = DataBlock::from(&mut data[..]);
            data_block.set_ttl(config.ttl_field_config.as_ref().unwrap(), 1);
            let associated_data = AssociatedData::new(data_block, config);

            assert_eq!(associated_data.get_stored_ttl_value().unwrap(), 1);

            // Associated data should be a snapshot
            // Further modifications should not change it
            data[0] = 7;
            assert_eq!(associated_data.get_stored_ttl_value().unwrap(), 1);
        }
    }

    #[test]
    fn lru_counter_access() {
        for bit_count in 1..=32 {
            // No additional features
            let config = CuckooConfiguration::builder(1000)
                .fingerprint_bits(5.try_into().unwrap())
                .with_lru(LruConfig {
                    counter_bits: bit_count.try_into().unwrap(),
                })
                .build()
                .unwrap();

            let mut data = [0u8; 5];
            let mut data_block = DataBlock::from(&mut data[..]);
            data_block.inc_lru_counter(config.lru_field_config.as_ref().unwrap());
            let associated_data = AssociatedData::new(data_block, config);

            assert_eq!(associated_data.get_lru_counter().unwrap(), 1);

            // Associated data should be a snapshot
            // Further modifications should not change it
            data[0] = 7;
            assert_eq!(associated_data.get_lru_counter().unwrap(), 1);
        }
    }
}
