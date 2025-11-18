use crate::config::{
    CounterConfig, CuckooConfiguration, CuckooConfigurationBuilder, LruConfig, TtlConfig,
};
use std::ops::{Range, RangeInclusive};

#[derive(Debug, Hash, Clone, PartialEq, Eq)]
pub(crate) struct Fingerprint {
    data: u32,
}

impl Fingerprint {
    pub(crate) fn new(hash: u32, mask: u32) -> Self {
        let mut fingerprint = hash & mask;
        if fingerprint == 0 {
            fingerprint = 1;
        }

        Self { data: fingerprint }
    }

    pub(crate) fn new_empty() -> Self {
        Self { data: 0 }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.data == 0
    }

    pub(crate) fn data(&self) -> u32 {
        self.data
    }
}

impl From<u32> for Fingerprint {
    fn from(value: u32) -> Self {
        Self { data: value }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct DataBlockFieldConfiguration {
    bits: Range<usize>,
    bytes: RangeInclusive<usize>,
    mask: u32,
    in_value_mask: u32,
}

impl DataBlockFieldConfiguration {
    pub(crate) fn new(bits: Range<usize>) -> Self {
        let start_byte = bits.start / 8; // Round down to take the lower byte
        let end_byte = (bits.end - 1) / 8;
        let bytes = start_byte..=end_byte;
        let len = end_byte - start_byte + 1;
        Self {
            bits: bits.clone(),
            bytes,
            mask: !((u32::MAX << (32 - len * 8)) >> (bits.start - start_byte * 8)),
            // TODO: Check if the +1 is needed?
            in_value_mask: (1u32 << (bits.len()/* + 1 */)) - 1,
        }
    }

    pub(crate) fn value_mask(&self) -> u32 {
        self.in_value_mask
    }
}

pub(crate) struct ReadOnlyDataBlock<'a>(&'a [u8]);
pub(crate) struct DataBlock<'a>(&'a mut [u8]);

impl<'a> From<&'a mut [u8]> for DataBlock<'a> {
    fn from(value: &'a mut [u8]) -> Self {
        Self(value)
    }
}

impl<'a> From<&'a [u8]> for ReadOnlyDataBlock<'a> {
    fn from(value: &'a [u8]) -> Self {
        Self(value)
    }
}

// TODO: traits
impl<'a> DataBlock<'a> {
    // Sum of bits will never reach the size of `usize`, so no need to do checked adds
    pub(crate) fn get_size(configuration: &CuckooConfigurationBuilder) -> usize {
        let mut bits = *configuration.fingerprint_bits;
        if let Some(LruConfig { counter_bits, .. }) = configuration.lru {
            bits += *counter_bits;
        }
        if let Some(TtlConfig { ttl_bits, .. }) = configuration.ttl {
            bits += *ttl_bits;
        }
        if let Some(CounterConfig { counter_bits, .. }) = configuration.counter {
            bits += *counter_bits;
        }
        bits.div_ceil(8)
    }

    pub(crate) fn inner(self) -> &'a mut [u8] {
        self.0
    }

    pub(crate) fn load_bits(&self, config: &DataBlockFieldConfiguration) -> u32 {
        let loaded = &self.0[config.bytes.clone()];
        let mut loaded_u32 = 0;
        let len = loaded.len();
        for (i, b) in loaded.iter().enumerate() {
            loaded_u32 += (*b as u32) << ((len - (i + 1)) * 8)
        }
        loaded_u32 & config.mask
    }

    pub(crate) fn store_bits(&mut self, config: &DataBlockFieldConfiguration, value: u32) {
        let masked_new_value = value & config.in_value_mask;
        let loaded = &self.0[config.bytes.clone()];
        let len = loaded.len();
        let mut loaded_u32 = 0;
        for (i, b) in loaded.iter().enumerate() {
            loaded_u32 += (*b as u32) << ((len - i) * 8)
        }
        let masked_old_value = loaded_u32 & config.mask;
        let final_value = masked_old_value | masked_new_value;
        self.0[config.bytes.clone()].copy_from_slice(&final_value.to_be_bytes()[(4 - len)..]);
    }

    pub(crate) fn get_fingerprint(&self, configuration: &CuckooConfiguration) -> Fingerprint {
        self.load_bits(&configuration.fingerprint_field_config)
            .into()
    }

    pub(crate) fn store_fingerprint(
        &mut self,
        fingerprint: &Fingerprint,
        configuration: &CuckooConfiguration,
    ) {
        self.store_bits(&configuration.fingerprint_field_config, fingerprint.data);
    }

    pub(crate) fn reset(&mut self) {
        let len = self.0.len();
        self.0[0..len].copy_from_slice(&vec![0u8; len]);
    }

    pub(crate) fn swap(&mut self, other: &mut DataBlock<'_>) {
        self.0.swap_with_slice(other.0);
    }

    #[allow(clippy::cast_possible_truncation)]
    pub(crate) fn get_lru_counter(
        &self,
        configuration: &(LruConfig, DataBlockFieldConfiguration),
    ) -> u8 {
        self.load_bits(&configuration.1) as u8
    }

    #[allow(clippy::cast_possible_truncation)]
    pub(crate) fn inc_lru_counter(
        &mut self,
        configuration: &(LruConfig, DataBlockFieldConfiguration),
    ) {
        let counter = self.load_bits(&configuration.1) as u8;
        self.store_bits(&configuration.1, (counter + 1) as u32);
    }

    #[allow(clippy::cast_possible_truncation)]
    pub(crate) fn age_lru_counter(
        &mut self,
        configuration: &(LruConfig, DataBlockFieldConfiguration),
    ) {
        let counter = self.load_bits(&configuration.1) as u8;
        self.store_bits(&configuration.1, (counter >> 1) as u32);
    }

    pub(crate) fn get_counter(
        &self,
        configuration: &(LruConfig, DataBlockFieldConfiguration),
    ) -> u32 {
        self.load_bits(&configuration.1)
    }

    pub(crate) fn inc_counter(
        &mut self,
        configuration: &(CounterConfig, DataBlockFieldConfiguration),
        by: u32,
    ) {
        let counter = self.load_bits(&configuration.1);
        self.store_bits(&configuration.1, counter.saturating_add(by));
    }

    pub(crate) fn dec_counter(
        &mut self,
        configuration: &(CounterConfig, DataBlockFieldConfiguration),
        by: u32,
    ) {
        let counter = self.load_bits(&configuration.1);
        self.store_bits(&configuration.1, counter.saturating_sub(by));
    }

    pub(crate) fn get_ttl(&self, configuration: &(TtlConfig, DataBlockFieldConfiguration)) -> u32 {
        self.load_bits(&configuration.1)
    }

    pub(crate) fn set_ttl(
        &mut self,
        configuration: &(TtlConfig, DataBlockFieldConfiguration),
        ttl: u32,
    ) {
        self.store_bits(&configuration.1, ttl);
    }
}

impl<'a> ReadOnlyDataBlock<'a> {
    pub(crate) fn load_bits(&self, config: &DataBlockFieldConfiguration) -> u32 {
        let loaded = &self.0[config.bytes.clone()];
        let mut loaded_u32 = 0;
        let len = loaded.len();
        for (i, b) in loaded.iter().enumerate() {
            loaded_u32 += (*b as u32) << ((len - (i + 1)) * 8)
        }
        loaded_u32 & config.mask
    }

    pub(crate) fn inner(self) -> &'a [u8] {
        self.0
    }

    pub(crate) fn get_fingerprint(&self, configuration: &CuckooConfiguration) -> Fingerprint {
        self.load_bits(&configuration.fingerprint_field_config)
            .into()
    }

    #[allow(clippy::cast_possible_truncation)]
    pub(crate) fn get_lru_counter(
        &self,
        configuration: &(LruConfig, DataBlockFieldConfiguration),
    ) -> u8 {
        self.load_bits(&configuration.1) as u8
    }

    pub(crate) fn get_counter(
        &self,
        configuration: &(CounterConfig, DataBlockFieldConfiguration),
    ) -> u32 {
        self.load_bits(&configuration.1)
    }

    pub(crate) fn get_ttl(&self, configuration: &(TtlConfig, DataBlockFieldConfiguration)) -> u32 {
        self.load_bits(&configuration.1)
    }
}
