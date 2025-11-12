use crate::filter::CuckooConfiguration;
use std::ops::Range;

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
}

impl From<u32> for Fingerprint {
    fn from(value: u32) -> Self {
        Self { data: value }
    }
}

pub(crate) struct DataBlock<'a>(&'a mut [u8]);

impl<'a> From<&'a mut [u8]> for DataBlock<'a> {
    fn from(value: &'a mut [u8]) -> Self {
        Self(value)
    }
}

impl<'a> DataBlock<'a> {
    pub(crate) fn get_size(configuration: &CuckooConfiguration) -> usize {
        let mut bits = configuration.fingerprint_bits;
        if configuration.lru_enabled {
            bits += 8;
        }
        if configuration.ttl_enabled {
            bits += configuration.ttl_bits;
        }
        if configuration.counter_enabled {
            bits += configuration.counter_bits;
        }
        bits.div_ceil(8)
    }

    pub(crate) fn load_bits(&self, bits: Range<usize>) -> u32 {
        let start_byte = bits.start / 8; // Round down to take the lower byte
        let end_byte = (bits.end - 1) / 8;
        let loaded = &self.0[start_byte..=end_byte];
        let mut loaded_u32 = 0;
        let len = loaded.len();
        for (i, b) in loaded.iter().enumerate() {
            loaded_u32 += (*b as u32) << ((len - (i + 1)) * 8)
        }
        let mask = !((u32::MAX << (32 - bits.len())) >> bits.start);
        loaded_u32 & mask
    }

    pub(crate) fn store_bits(&mut self, bits: Range<usize>, value: u32) {
        let val_mask = (1u32 << (bits.len() + 1)) - 1;
        let masked_new_value = value & val_mask;
        let start_byte = bits.start / 8; // Round down to take the lower byte
        let end_byte = (bits.end - 1) / 8;
        let loaded = &self.0[start_byte..=end_byte];
        let len = loaded.len();
        let mask = !((u32::MAX << (32 - len * 8)) >> (bits.start - start_byte * 8));
        let mut loaded_u32 = 0;
        for (i, b) in loaded.iter().enumerate() {
            loaded_u32 += (*b as u32) << ((len - i) * 8)
        }
        let masked_old_value = loaded_u32 & mask;
        let final_value = masked_old_value | masked_new_value;
        self.0[start_byte..=end_byte].copy_from_slice(&final_value.to_be_bytes()[(4 - len)..]);
    }

    pub(crate) fn get_fingerprint(&self, configuration: &CuckooConfiguration) -> Fingerprint {
        self.load_bits(0..configuration.fingerprint_bits).into()
    }

    pub(crate) fn store_fingerprint(
        &mut self,
        fingerprint: &Fingerprint,
        configuration: &CuckooConfiguration,
    ) {
        self.store_bits(0..configuration.fingerprint_bits, fingerprint.data);
    }

    pub(crate) fn reset(&mut self) {
        let len = self.0.len();
        self.0[0..len].copy_from_slice(&vec![0u8; len]);
    }

    pub(crate) fn swap(&mut self, other: &mut DataBlock<'_>) {
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
            std::ptr::swap_nonoverlapping(self.0.as_mut_ptr(), other.0.as_mut_ptr(), self.0.len());
        }
    }

    pub(crate) fn get_lru_counter(&self, configuration: &CuckooConfiguration) -> u8 {
        self.load_bits(configuration.fingerprint_bits..configuration.fingerprint_bits + 8) as u8
    }

    pub(crate) fn inc_lru_counter(&mut self, configuration: &CuckooConfiguration) {
        let counter_range = configuration.fingerprint_bits..configuration.fingerprint_bits + 8;
        let counter = self.load_bits(counter_range.clone()) as u8;
        self.store_bits(counter_range, (counter + 1) as u32);
    }

    pub(crate) fn age_lru_counter(&mut self, configuration: &CuckooConfiguration) {
        let counter_range = configuration.fingerprint_bits..configuration.fingerprint_bits + 8;
        let counter = self.load_bits(counter_range.clone()) as u8;
        self.store_bits(counter_range, (counter >> 1) as u32);
    }

    pub(crate) fn get_counter(&self, configuration: &CuckooConfiguration) -> u32 {
        let mut counter_start = configuration.fingerprint_bits;
        if configuration.lru_enabled {
            counter_start += 8;
        }
        if configuration.ttl_enabled {
            counter_start += configuration.ttl_bits;
        }
        self.load_bits((counter_start..counter_start + configuration.counter_bits).clone())
    }

    pub(crate) fn inc_counter(&mut self, configuration: &CuckooConfiguration, by: u32) {
        let mut counter_start = configuration.fingerprint_bits;
        if configuration.lru_enabled {
            counter_start += 8;
        }
        if configuration.ttl_enabled {
            counter_start += configuration.ttl_bits;
        }
        let counter_range = counter_start..counter_start + configuration.counter_bits;
        let counter = self.load_bits(counter_range.clone());
        self.store_bits(counter_range, counter.saturating_add(by));
    }

    pub(crate) fn dec_counter(&mut self, configuration: &CuckooConfiguration, by: u32) {
        let mut counter_start = configuration.fingerprint_bits;
        if configuration.lru_enabled {
            counter_start += 8;
        }
        if configuration.ttl_enabled {
            counter_start += configuration.ttl_bits;
        }
        let counter_range = counter_start..counter_start + configuration.counter_bits;
        let counter = self.load_bits(counter_range.clone());
        self.store_bits(counter_range, counter.saturating_sub(by));
    }

    pub(crate) fn get_ttl(&self, configuration: &CuckooConfiguration) -> u32 {
        let mut ttl_start = configuration.fingerprint_bits;
        if configuration.lru_enabled {
            ttl_start += 8;
        }
        self.load_bits(ttl_start..ttl_start + configuration.ttl_bits)
    }

    pub(crate) fn set_ttl(&mut self, configuration: &CuckooConfiguration, ttl: u32) {
        let mut ttl_start = configuration.fingerprint_bits;
        if configuration.lru_enabled {
            ttl_start += 8;
        }
        self.store_bits(ttl_start..ttl_start + configuration.ttl_bits, ttl);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compare_fingerprint_to_bits() {
        let configuration = CuckooConfiguration {
            fingerprint_bits: 8,
            bucket_size: 4,
            max_entries: 1000,
            max_kicks: 500,
            lru_enabled: false,
            ttl_enabled: false,
            ttl: 100,
            ttl_bits: 0,
            ttl_resolution: 10,
            counter_enabled: false,
            counter_bits: 10,
        };
        let fingerprint_mask = (1u32 << configuration.fingerprint_bits) - 1;
        let fp = Fingerprint::new(137, fingerprint_mask);

        let mut data = [0u8; 4];

        let mut block: DataBlock<'_> = (&mut data[0..4]).into();

        block.store_fingerprint(&fp, &configuration);

        assert_eq!(block.load_bits(0..8), 137);

        block.reset();

        assert_eq!(block.load_bits(0..8), 0);
        assert_eq!(block.get_fingerprint(&configuration).data, 0);

        block.store_bits(0..8, 137);

        assert_eq!(block.get_fingerprint(&configuration), fp);
    }
}
