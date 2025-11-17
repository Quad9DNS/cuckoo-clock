use crate::filter::{CuckooConfiguration, DerivedConfiguration};
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

#[derive(Clone)]
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
    pub(crate) fn get_size(configuration: &CuckooConfiguration) -> usize {
        let mut bits = *configuration.fingerprint_bits;
        if configuration.lru_enabled {
            bits += 8;
        }
        if configuration.ttl_enabled {
            bits += *configuration.ttl_bits;
        }
        if configuration.counter_enabled {
            bits += *configuration.counter_bits;
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

    pub(crate) fn get_fingerprint(&self, derived: &DerivedConfiguration) -> Fingerprint {
        self.load_bits(&derived.fingerprint_field_config).into()
    }

    pub(crate) fn store_fingerprint(
        &mut self,
        fingerprint: &Fingerprint,
        derived: &DerivedConfiguration,
    ) {
        self.store_bits(&derived.fingerprint_field_config, fingerprint.data);
    }

    pub(crate) fn reset(&mut self) {
        let len = self.0.len();
        self.0[0..len].copy_from_slice(&vec![0u8; len]);
    }

    pub(crate) fn swap(&mut self, other: &mut DataBlock<'_>) {
        self.0.swap_with_slice(other.0);
    }

    #[allow(clippy::cast_possible_truncation)]
    pub(crate) fn get_lru_counter(&self, derived: &DerivedConfiguration) -> u8 {
        self.load_bits(&derived.lru_field_config) as u8
    }

    #[allow(clippy::cast_possible_truncation)]
    pub(crate) fn inc_lru_counter(&mut self, derived: &DerivedConfiguration) {
        let counter = self.load_bits(&derived.lru_field_config) as u8;
        self.store_bits(&derived.lru_field_config, (counter + 1) as u32);
    }

    #[allow(clippy::cast_possible_truncation)]
    pub(crate) fn age_lru_counter(&mut self, derived: &DerivedConfiguration) {
        let counter = self.load_bits(&derived.lru_field_config) as u8;
        self.store_bits(&derived.lru_field_config, (counter >> 1) as u32);
    }

    pub(crate) fn get_counter(&self, derived: &DerivedConfiguration) -> u32 {
        self.load_bits(&derived.counter_field_config)
    }

    pub(crate) fn inc_counter(&mut self, derived: &DerivedConfiguration, by: u32) {
        let counter = self.load_bits(&derived.counter_field_config);
        self.store_bits(&derived.counter_field_config, counter.saturating_add(by));
    }

    pub(crate) fn dec_counter(&mut self, derived: &DerivedConfiguration, by: u32) {
        let counter = self.load_bits(&derived.counter_field_config);
        self.store_bits(&derived.counter_field_config, counter.saturating_sub(by));
    }

    pub(crate) fn get_ttl(&self, derived: &DerivedConfiguration) -> u32 {
        self.load_bits(&derived.ttl_field_config)
    }

    pub(crate) fn set_ttl(&mut self, derived: &DerivedConfiguration, ttl: u32) {
        self.store_bits(&derived.ttl_field_config, ttl);
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

    pub(crate) fn get_fingerprint(&self, derived: &DerivedConfiguration) -> Fingerprint {
        self.load_bits(&derived.fingerprint_field_config).into()
    }

    #[allow(clippy::cast_possible_truncation)]
    pub(crate) fn get_lru_counter(&self, derived: &DerivedConfiguration) -> u8 {
        self.load_bits(&derived.lru_field_config) as u8
    }

    pub(crate) fn get_counter(&self, derived: &DerivedConfiguration) -> u32 {
        self.load_bits(&derived.counter_field_config)
    }

    pub(crate) fn get_ttl(&self, derived: &DerivedConfiguration) -> u32 {
        self.load_bits(&derived.ttl_field_config)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn compare_fingerprint_to_bits() {
        let configuration = CuckooConfiguration {
            fingerprint_bits: 8.try_into().unwrap(),
            bucket_size: 4,
            max_entries: 1000,
            max_kicks: 500,
            lru_enabled: false,
            ttl_enabled: false,
            ttl: 100,
            ttl_bits: 8.try_into().unwrap(),
            ttl_resolution: 10,
            counter_enabled: false,
            counter_bits: 10.try_into().unwrap(),
        };
        let derived = DerivedConfiguration::derive(&configuration);
        let fingerprint_mask = (1u32 << *configuration.fingerprint_bits) - 1;
        let fp = Fingerprint::new(137, fingerprint_mask);

        let mut data = [0u8; 4];

        let mut block: DataBlock<'_> = (&mut data[0..4]).into();

        block.store_fingerprint(&fp, &derived);

        assert_eq!(block.load_bits(&derived.fingerprint_field_config), 137);

        block.reset();

        assert_eq!(block.load_bits(&derived.fingerprint_field_config), 0);
        assert_eq!(block.get_fingerprint(&derived).data, 0);

        block.store_bits(&derived.fingerprint_field_config, 137);

        assert_eq!(block.get_fingerprint(&derived), fp);
    }
}
