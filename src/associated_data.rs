use std::{borrow::Borrow, fmt::Display};

use crate::{config::CuckooConfiguration, data_block::DataBlock};

/// Error type for all [`AssociatedData`] access.
#[derive(Debug)]
pub enum AccessError {
    /// Error due to requesting a field that is available only if a feature is enabled in
    /// [`crate::config::CuckooConfiguration`].
    FeatureNotEnabled(String),
}

impl Display for AccessError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AccessError::FeatureNotEnabled(feature) => {
                f.write_str(&format!("Feature ({feature}) not enabled."))
            }
        }
    }
}

impl std::error::Error for AccessError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        None
    }
}

/// Provides access to data associated with an item in the filter.
///
/// All data is associated by a fingerprint, meaning that collisions (false positives) will also
/// affect the associated data - it might not be related exactly to the requested item, but just to
/// another item that shared the same fingerprint.
///
/// This data is a copy of the data in the filter, meaning it will not be updated when filter data
/// is changed and can be freely moved around.
///
/// # Examples
///
/// ```
/// use cuckoo_clock::{CuckooFilter, config::{CuckooConfiguration, LruConfig, TtlConfig}};
///
/// let filter = CuckooFilter::new_random(
///     CuckooConfiguration::builder(100_000)
///         .with_lru(LruConfig::default())
///         .with_ttl(TtlConfig {
///             ttl: 10.try_into()?,
///             ttl_bits: 8.try_into()?,
///         })
///         .build()?
/// );
/// filter.insert("example_data");
/// let data = filter.get_associated_data("example_data").unwrap();
///
/// assert_eq!(data.get_stored_ttl_value()?, 10);
/// // `get_associated_data` also increases LRU counter - it is counted as an access
/// assert_eq!(data.get_lru_counter()?, 2);
///
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
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

    /// Returns the fingerprint for this item.
    ///
    /// Generally fingerprint is not very useful on its own, depending on the hasher used for
    /// [`crate::CuckooFilter`].
    #[must_use]
    pub fn get_fingerprint(&self) -> u32 {
        DataBlock::from(&self.data[..])
            .get_fingerprint(&self.configuration)
            .data()
    }

    /// Returns the LRU counter for this item.
    pub fn get_lru_counter(&self) -> Result<u32, AccessError> {
        Ok(DataBlock::from(&self.data[..]).get_lru_counter(
            self.configuration
                .lru_field_config
                .as_ref()
                .ok_or(AccessError::FeatureNotEnabled("LRU".to_string()))?,
        ))
    }

    /// Returns the generic counter for this item.
    pub fn get_counter(&self) -> Result<u32, AccessError> {
        Ok(DataBlock::from(&self.data[..]).get_counter(
            self.configuration
                .counter_field_config
                .as_ref()
                .ok_or(AccessError::FeatureNotEnabled("Counter".to_string()))?,
        ))
    }

    /// Returns the stored TTL value for this item.
    ///
    /// This is not a time to live in seconds. This is just a TTL counter, that is decremented by 1
    /// each time [`crate::CuckooFilter::scan_and_update_full`] is called, until it reaches 0.
    pub fn get_stored_ttl_value(&self) -> Result<u32, AccessError> {
        Ok(DataBlock::from(&self.data[..]).get_ttl(
            self.configuration
                .ttl_field_config
                .as_ref()
                .ok_or(AccessError::FeatureNotEnabled("TTL".to_string()))?,
        ))
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used)]
mod tests {
    use crate::{
        Fingerprint,
        config::{LruConfig, TtlConfig},
    };

    use super::*;

    #[test]
    #[allow(unused_assignments)]
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

        let Err(AccessError::FeatureNotEnabled(feature_name)) =
            associated_data.get_stored_ttl_value()
        else {
            panic!("TTL not enabled error should be returned!");
        };
        assert_eq!(feature_name, "TTL");

        let Err(AccessError::FeatureNotEnabled(feature_name)) = associated_data.get_lru_counter()
        else {
            panic!("LRU not enabled error should be returned!");
        };
        assert_eq!(feature_name, "LRU");

        let Err(AccessError::FeatureNotEnabled(feature_name)) = associated_data.get_counter()
        else {
            panic!("Counter not enabled error should be returned!");
        };
        assert_eq!(feature_name, "Counter");
    }

    #[test]
    #[allow(unused_assignments)]
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
    #[allow(unused_assignments)]
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
