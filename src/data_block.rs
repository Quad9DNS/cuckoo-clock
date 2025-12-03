use crate::{
    config::{BitCount, CounterConfig, CuckooConfiguration, LruConfig, TtlConfig},
    filter::CuckooFilter,
};
use std::{
    borrow::Borrow,
    hash::{BuildHasher, Hash},
};
use std::{
    borrow::BorrowMut,
    ops::{Range, RangeInclusive},
};

/// Represents a fingerprint of an item, stored in the [`crate::CuckooFilter`].
///
/// Generally, fingerprint access should not be needed for regular filter usage, but it can be
/// useful to confirm if the evicted fingerprint matches a known item.
///
/// # Examples
///
/// ```ignore
/// use cuckoo_clock::{CuckooFilter, Fingerprint, config::CuckooConfiguration};
///
/// let filter = CuckooFilter::new_random(CuckooConfiguration::builder(100_000).build()?);
///
/// // ... use filter
///
/// if let Some(evicted_fp: Fingerprint) = filter.insert("new_item") {
///     if evicted_fp.matches_key("known_item", &filter) {
///         println!("Known item was evicted!");
///     }
/// }
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Hash, Clone, PartialEq, Eq)]
pub struct Fingerprint {
    data: u32,
}

impl Fingerprint {
    pub(crate) const fn new(hash: u32, mask: u32) -> Self {
        let mut fingerprint = hash & mask;
        // Since fingerprint 0 represents and empty slot, we just shift it up by 1
        // This will result in slightly higher collision rate
        if fingerprint == 0 {
            fingerprint = 1;
        }

        Self { data: fingerprint }
    }

    /// Returns true if this represents and empty fingerprint (an empty slot).
    pub(crate) const fn is_empty(&self) -> bool {
        self.data == 0
    }

    /// Returns the underlying data.
    pub(crate) const fn data(&self) -> u32 {
        self.data
    }

    /// Compares this fingerprint to a key. This will only produce correct results if the same
    /// filter that produced this fingerprint is passed into this method.
    ///
    /// Returns true if this fingerprint matches the key.
    pub fn matches_key<K: Hash + ?Sized, H: BuildHasher>(
        &self,
        key: &K,
        filter: &CuckooFilter<H>,
    ) -> bool {
        filter.get_fingerprint(key) == *self
    }
}

impl From<u32> for Fingerprint {
    fn from(value: u32) -> Self {
        Self { data: value }
    }
}

/// Configuration for a field in a [`DataBlock`] (a single slot in a bucket).
#[derive(Clone, Debug)]
pub(crate) struct DataBlockFieldConfiguration {
    /// Range of bytes that contain this field.
    /// Not all bytes are actually exclusive to this field.
    /// The specific bits that are used for this field are accessed using
    /// [`DataBlockFieldConfiguration::mask`].
    bytes: RangeInclusive<usize>,
    /// Mask for this field. When applied to [`DataBlockFieldConfiguration::bytes`] range of bytes
    /// from the data block, stored field can be retrieved.
    // Mask has to be u64, because even though max value can fit in u32, it can spread over 4 bytes
    // due to layout
    mask: u64,
    /// Since the field doesn't have to be aligned at byte, shift is used to correct the value
    /// after masking.
    ///
    /// A simplified example using a shorter mask would be:
    /// mask = 00111100
    ///
    /// In this case, shift = 2 and can be used after masking to move masked bits into correct
    /// position.
    shift: usize,
    /// Mask for incoming values. Maximum bits is [`BitCount::MAX`] (32), so u32 is big enough.
    /// This is used to mask incoming values, to ensure they don't overflow into other
    /// fields when writing into the [`DataBlock`].
    in_value_mask: u32,
}

impl DataBlockFieldConfiguration {
    /// Creates a new [`DataBlockFieldConfiguration`] from the range of bits that it takes up. Bit
    /// indexes start from the beginning of the [`DataBlock`], meaning from the start of the first
    /// field (which will always be the [`Fingerprint`]). It is limited to
    /// [`BitCount::MAX`]
    pub(crate) fn new(bits: Range<usize>) -> Self {
        // This should be handled at `BitCount` validation
        debug_assert!(bits.len() <= BitCount::MAX.into());
        let start_byte = bits.start / 8; // Round down to take the lower byte
        let end_byte = (bits.end - 1) / 8;
        let bytes = start_byte..=end_byte;
        let shift = (end_byte + 1) * 8 - bits.end;
        Self {
            bytes,
            shift,
            mask: ((1u64 << bits.len()) - 1) << shift,
            // u64 is used to prevent overflow when bits.len() == 32
            // The final value will be at most u32::MAX, since bits.len() is limited to be <= 32
            #[allow(clippy::cast_possible_truncation)]
            in_value_mask: ((1u64 << bits.len()) - 1) as u32,
        }
    }

    /// Mask for incoming values. Maximum bits is [`crate::config::BitCount::MAX`] (32), so u32 is
    /// big enough. This is used to mask incoming values, to ensure they don't overflow into other
    /// fields when writing into the [`DataBlock`].
    pub(crate) const fn value_mask(&self) -> u32 {
        self.in_value_mask
    }
}

/// Simple wrapper for a slot in a bucket. Enables access to the fields stored in the slot.
pub(crate) struct DataBlock<T: Borrow<[u8]>>(T);

impl<'a> From<&'a mut [u8]> for DataBlock<&'a mut [u8]> {
    fn from(value: &'a mut [u8]) -> Self {
        Self(value)
    }
}

impl<'a> From<&'a [u8]> for DataBlock<&'a [u8]> {
    fn from(value: &'a [u8]) -> Self {
        Self(value)
    }
}

impl<T: Borrow<[u8]>> DataBlock<T> {
    /// Provides read-only access to underlying bytes.
    pub(crate) fn inner(&self) -> &[u8] {
        self.0.borrow()
    }

    /// Loads bits defined by the [`DataBlockFieldConfiguration`] and converts them to [`u32`].
    #[allow(clippy::cast_possible_truncation)]
    pub(crate) fn load_bits(&self, config: &DataBlockFieldConfiguration) -> u32 {
        let loaded = &self.0.borrow()[config.bytes.clone()];
        let mut loaded_u64 = 0;
        let len = loaded.len();
        for (i, b) in loaded.iter().enumerate() {
            loaded_u64 += (*b as u64) << ((len - (i + 1)) * 8)
        }
        ((loaded_u64 & config.mask) >> config.shift) as u32
    }

    /// Loads fingerprint based on fingerprint configuration in the [`CuckooConfiguration`].
    pub(crate) fn get_fingerprint(&self, configuration: &CuckooConfiguration) -> Fingerprint {
        self.load_bits(&configuration.fingerprint_field_config)
            .into()
    }

    /// Loads LRU based on the provided [`DataBlockFieldConfiguration`].
    pub(crate) fn get_lru_counter(
        &self,
        configuration: &(LruConfig, DataBlockFieldConfiguration),
    ) -> u32 {
        self.load_bits(&configuration.1)
    }

    /// Loads generic counter based on the provided [`DataBlockFieldConfiguration`].
    pub(crate) fn get_counter(
        &self,
        configuration: &(CounterConfig, DataBlockFieldConfiguration),
    ) -> u32 {
        self.load_bits(&configuration.1)
    }

    /// Loads TTL based on the provided [`DataBlockFieldConfiguration`].
    pub(crate) fn get_ttl(&self, configuration: &(TtlConfig, DataBlockFieldConfiguration)) -> u32 {
        self.load_bits(&configuration.1)
    }
}

impl<T: BorrowMut<[u8]>> DataBlock<T> {
    /// Stores the value as bits defined by the [`DataBlockFieldConfiguration`].
    pub(crate) fn store_bits(&mut self, config: &DataBlockFieldConfiguration, value: u32) {
        let masked_new_value = value & config.in_value_mask;
        let loaded = &self.0.borrow()[config.bytes.clone()];
        let len = loaded.len();
        let mut loaded_u64 = 0;
        for (i, b) in loaded.iter().enumerate() {
            loaded_u64 += (*b as u64) << ((len - (i + 1)) * 8)
        }
        #[allow(clippy::cast_possible_truncation)]
        let masked_old_value = loaded_u64 & !config.mask;
        let final_value = masked_old_value | ((masked_new_value as u64) << config.shift);
        self.0.borrow_mut()[config.bytes.clone()]
            .copy_from_slice(&final_value.to_be_bytes()[(8 - len)..]);
    }

    /// Stores the fingerprint based on fingerprint configuration in the [`CuckooConfiguration`].
    pub(crate) fn store_fingerprint(
        &mut self,
        fingerprint: &Fingerprint,
        configuration: &CuckooConfiguration,
    ) {
        self.store_bits(&configuration.fingerprint_field_config, fingerprint.data);
    }

    /// Resets this data block by setting all bits to 0.
    pub(crate) fn reset(&mut self) {
        self.0.borrow_mut().fill(0u8);
    }

    /// Swaps this data block with another data block.
    ///
    /// # Panics
    ///
    /// Panics if the data blocks have different lengths. Both data blocks should come from the
    /// same filter.
    pub(crate) fn swap<U: BorrowMut<[u8]>>(&mut self, other: &mut DataBlock<U>) {
        self.0.borrow_mut().swap_with_slice(other.0.borrow_mut());
    }

    /// Increments the LRU counter, based on the provided [`DataBlockFieldConfiguration`].
    pub(crate) fn inc_lru_counter(
        &mut self,
        configuration: &(LruConfig, DataBlockFieldConfiguration),
    ) {
        let counter = self.load_bits(&configuration.1);
        let mut new_counter = counter.saturating_add(1);
        // Value mask is also the max possible value
        if new_counter > configuration.1.value_mask() {
            new_counter = configuration.1.value_mask();
        }
        self.store_bits(&configuration.1, new_counter);
    }

    /// Ages the LRU counter, based on the provided [`DataBlockFieldConfiguration`].
    ///
    /// Aging is done by halving the LRU counter value.
    pub(crate) fn age_lru_counter(
        &mut self,
        configuration: &(LruConfig, DataBlockFieldConfiguration),
    ) {
        let counter = self.load_bits(&configuration.1);
        self.store_bits(&configuration.1, counter >> 1);
    }

    /// Ages the TTL counter, based on the provided [`DataBlockFieldConfiguration`] and clears out
    /// the data if it reaches 0.
    ///
    /// Aging is done by reducing the TTL counter by 1.
    pub(crate) fn age_ttl_counter(
        &mut self,
        configuration: &(TtlConfig, DataBlockFieldConfiguration),
    ) {
        let mut counter = self.load_bits(&configuration.1);
        counter = counter.saturating_sub(1);
        self.store_bits(&configuration.1, counter);
        if counter == 0 {
            self.reset();
        }
    }

    /// Increments the generic counter, based on the provided [`DataBlockFieldConfiguration`], by
    /// the provided value.
    pub(crate) fn inc_counter(
        &mut self,
        configuration: &(CounterConfig, DataBlockFieldConfiguration),
        by: u32,
    ) {
        let counter = self.load_bits(&configuration.1);
        let mut new_counter = counter.saturating_add(by);
        // Value mask is also the max possible value
        if new_counter > configuration.1.value_mask() {
            new_counter = configuration.1.value_mask();
        }
        self.store_bits(&configuration.1, new_counter);
    }

    /// Decrements the generic counter, based on the provided [`DataBlockFieldConfiguration`], by
    /// the provided value.
    pub(crate) fn dec_counter(
        &mut self,
        configuration: &(CounterConfig, DataBlockFieldConfiguration),
        by: u32,
    ) {
        let counter = self.load_bits(&configuration.1);
        self.store_bits(&configuration.1, counter.saturating_sub(by));
    }

    /// Sets the TTL counter, based on the provided [`DataBlockFieldConfiguration`].
    pub(crate) fn set_ttl(
        &mut self,
        configuration: &(TtlConfig, DataBlockFieldConfiguration),
        ttl: u32,
    ) {
        self.store_bits(&configuration.1, ttl);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn test_load_store_for_each_bit_count() {
        let mut data = [0u8; 4];
        let mut data_block = DataBlock::from(&mut data[..]);
        for i in 1usize..=32usize {
            let field_config = DataBlockFieldConfiguration::new(0..i);
            // Ensure we are using the max value possible for set bit count
            let value: u32 = ((1u64 << i) - 1).try_into().unwrap();
            data_block.reset();
            data_block.store_bits(&field_config, value);
            assert_eq!(
                data_block.load_bits(&field_config),
                value,
                "Loaded value was different for bit count {i}"
            );
        }
    }

    #[test]
    fn test_load_store_at_non_byte_boundary() {
        // Let's assume we have 13 bit fingerprint and additional data after it
        let data_start = 13;
        let fp_config = DataBlockFieldConfiguration::new(0..data_start);
        let fp_value = 0b1010101010101u32;
        let mut data = [0u8; 7];
        let mut data_block = DataBlock::from(&mut data[..]);
        data_block.store_bits(&fp_config, fp_value);
        for i in 1usize..=32usize {
            let field_config = DataBlockFieldConfiguration::new(data_start..data_start + i);
            // Ensure we are using the max value possible for set bit count
            let value: u32 = ((1u64 << i) - 1).try_into().unwrap();
            data_block.store_bits(&field_config, 0);
            data_block.store_bits(&field_config, value);
            assert_eq!(
                data_block.load_bits(&field_config),
                value,
                "Loaded value was different for bit count {i}"
            );
        }
        assert_eq!(
            data_block.load_bits(&fp_config),
            fp_value,
            "Loads/stores wrote outside of their bits"
        );
    }

    #[test]
    fn test_load_store_full_config() {
        let config = CuckooConfiguration::builder(100_000)
            .fingerprint_bits(32.try_into().unwrap())
            .with_lru(LruConfig::default())
            .with_ttl(TtlConfig {
                ttl: 10.try_into().unwrap(),
                ttl_bits: 4.try_into().unwrap(),
            })
            .build()
            .unwrap();
        let mut data = [0u8; 6];
        let mut data_block = DataBlock::from(&mut data[..]);
        let fp_value = 0b1010101010101u32;

        data_block.store_fingerprint(&Fingerprint { data: fp_value }, &config);
        data_block.inc_lru_counter(config.lru_field_config.as_ref().unwrap());
        data_block.set_ttl(config.ttl_field_config.as_ref().unwrap(), 10);

        data_block.age_lru_counter(config.lru_field_config.as_ref().unwrap());
        data_block.age_ttl_counter(config.ttl_field_config.as_ref().unwrap());

        assert_eq!(data_block.get_fingerprint(&config).data(), fp_value);
        assert_eq!(
            data_block.get_ttl(config.ttl_field_config.as_ref().unwrap()),
            9
        );
        assert_eq!(
            data_block.get_lru_counter(config.lru_field_config.as_ref().unwrap()),
            0
        );
    }

    #[test]
    fn test_inc_counter_saturation() {
        let config = CuckooConfiguration::builder(100_000)
            .fingerprint_bits(8.try_into().unwrap())
            .with_lru(LruConfig {
                counter_bits: 2.try_into().unwrap(),
            })
            .build()
            .unwrap();

        let mut data = [0u8; 2];
        let mut data_block = DataBlock::from(&mut data[..]);

        let lru_config = config.lru_field_config.as_ref().unwrap();

        data_block.inc_lru_counter(lru_config);
        data_block.inc_lru_counter(lru_config);
        data_block.inc_lru_counter(lru_config);

        assert_eq!(data_block.get_lru_counter(lru_config), 3);

        // Since counter is limited at 2 bits, it shouldn't be able to go over 3
        data_block.inc_lru_counter(lru_config);
        assert_eq!(data_block.get_lru_counter(lru_config), 3);

        data_block.age_lru_counter(lru_config);
        assert_eq!(data_block.get_lru_counter(lru_config), 1);
    }
}
