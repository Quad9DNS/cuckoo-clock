use std::{
    num::NonZeroU32,
    ops::{Add, Deref, DerefMut},
};

use crate::data_block::DataBlockFieldConfiguration;

pub struct CuckooConfigurationBuilder {
    pub(crate) fingerprint_bits: BitCount,
    pub(crate) bucket_size: usize,
    pub(crate) max_entries: usize,
    pub(crate) max_kicks: usize,
    pub(crate) lru: Option<LruConfig>,
    pub(crate) ttl: Option<TtlConfig>,
    pub(crate) counter: Option<CounterConfig>,
}

impl CuckooConfigurationBuilder {
    pub const fn fingerprint_bits(&mut self, bits: BitCount) -> &mut Self {
        self.fingerprint_bits = bits;
        self
    }

    pub const fn bucket_size(&mut self, size: usize) -> &mut Self {
        self.bucket_size = size;
        self
    }

    pub const fn max_kicks(&mut self, kicks: usize) -> &mut Self {
        self.max_kicks = kicks;
        self
    }

    pub const fn with_lru(&mut self, lru: LruConfig) -> &mut Self {
        self.lru = Some(lru);
        self
    }

    pub const fn with_ttl(&mut self, ttl: TtlConfig) -> &mut Self {
        self.ttl = Some(ttl);
        self
    }

    pub fn build(&self) -> crate::Result<CuckooConfiguration> {
        let required_bucket_count = self.max_entries.div_ceil(self.bucket_size);
        let bucket_count = required_bucket_count.next_power_of_two();
        let ttl_start = *self.fingerprint_bits
            + if let Some(LruConfig { counter_bits, .. }) = self.lru {
                *counter_bits
            } else {
                0
            };
        let counter_start = ttl_start
            + if let Some(TtlConfig { ttl_bits, .. }) = self.ttl {
                *ttl_bits
            } else {
                0
            };

        // Sum of bits will never reach the size of `usize`, so no need to do checked adds
        let mut data_block_size = *self.fingerprint_bits;
        if let Some(LruConfig { counter_bits, .. }) = self.lru {
            data_block_size += *counter_bits;
        }
        if let Some(TtlConfig { ttl_bits, .. }) = self.ttl {
            data_block_size += *ttl_bits;
        }
        if let Some(CounterConfig { counter_bits, .. }) = self.counter {
            data_block_size += *counter_bits;
        }
        data_block_size = data_block_size.div_ceil(8);
        Ok(CuckooConfiguration {
            bucket_size: self.bucket_size,
            max_kicks: self.max_kicks,

            fingerprint_field_config: DataBlockFieldConfiguration::new(0..*self.fingerprint_bits),
            lru_field_config: self.lru.clone().map(|lru| {
                (
                    lru,
                    DataBlockFieldConfiguration::new(
                        *self.fingerprint_bits
                            ..*self.fingerprint_bits
                                + self
                                    .lru
                                    .as_ref()
                                    .map(|l| l.counter_bits)
                                    .unwrap_or(BitCount(0)),
                    ),
                )
            }),
            ttl_field_config: self.ttl.clone().map(|ttl| {
                (
                    ttl,
                    DataBlockFieldConfiguration::new(
                        ttl_start
                            ..ttl_start
                                + *self.ttl.as_ref().map(|t| t.ttl_bits).unwrap_or(BitCount(0)),
                    ),
                )
            }),
            counter_field_config: self.counter.clone().map(|counter| {
                (
                    counter,
                    DataBlockFieldConfiguration::new(
                        counter_start
                            ..counter_start
                                + *self
                                    .counter
                                    .as_ref()
                                    .map(|c| c.counter_bits)
                                    .unwrap_or(BitCount(0)),
                    ),
                )
            }),
            data_block_size,
            bucket_byte_size: self
                .bucket_size
                .checked_mul(data_block_size)
                .ok_or(crate::Error::BucketTooBig)?,
            bucket_count,
            #[allow(clippy::cast_possible_truncation)]
            buckets_mask: (bucket_count - 1) as u32,
        })
    }
}

#[derive(Clone, Debug)]
pub struct LruConfig {
    pub counter_bits: BitCount,
}

impl Default for LruConfig {
    fn default() -> Self {
        Self {
            counter_bits: BitCount(8),
        }
    }
}

#[derive(Clone, Debug)]
pub struct TtlConfig {
    pub ttl: NonZeroU32,
    pub ttl_bits: BitCount,
}

#[derive(Clone, Debug)]
pub struct CounterConfig {
    pub counter_bits: BitCount,
}

impl Default for CounterConfig {
    fn default() -> Self {
        Self {
            counter_bits: BitCount(8),
        }
    }
}

#[derive(Clone, Debug)]
pub struct CuckooConfiguration {
    pub(crate) bucket_size: usize,
    pub(crate) max_kicks: usize,

    pub(crate) fingerprint_field_config: DataBlockFieldConfiguration,
    pub(crate) lru_field_config: Option<(LruConfig, DataBlockFieldConfiguration)>,
    pub(crate) counter_field_config: Option<(CounterConfig, DataBlockFieldConfiguration)>,
    pub(crate) ttl_field_config: Option<(TtlConfig, DataBlockFieldConfiguration)>,
    pub(crate) data_block_size: usize,
    pub(crate) bucket_byte_size: usize,
    pub(crate) bucket_count: usize,
    pub(crate) buckets_mask: u32,
}

impl CuckooConfiguration {
    #[must_use]
    pub const fn builder(max_entries: usize) -> CuckooConfigurationBuilder {
        CuckooConfigurationBuilder {
            fingerprint_bits: BitCount(8),
            bucket_size: 4,
            max_entries,
            max_kicks: 500,
            lru: None,
            ttl: None,
            counter: None,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct BitCount(usize);

impl Deref for BitCount {
    type Target = usize;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for BitCount {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl TryFrom<usize> for BitCount {
    type Error = crate::Error;

    fn try_from(value: usize) -> Result<Self, Self::Error> {
        if value > 32 {
            return Err(crate::error::Error::BitCountTooHigh);
        }
        Ok(Self(value))
    }
}

impl From<BitCount> for usize {
    fn from(value: BitCount) -> Self {
        value.0
    }
}

// Since bit count can't be higher than 32
// Conversion into any integer is fine
impl From<BitCount> for u64 {
    fn from(value: BitCount) -> Self {
        value.0 as u64
    }
}

impl From<BitCount> for u32 {
    #[allow(clippy::cast_possible_truncation)]
    fn from(value: BitCount) -> Self {
        value.0 as u32
    }
}

impl From<BitCount> for u16 {
    #[allow(clippy::cast_possible_truncation)]
    fn from(value: BitCount) -> Self {
        value.0 as u16
    }
}

impl Add<usize> for BitCount {
    type Output = usize;

    fn add(self, rhs: usize) -> Self::Output {
        self.0 + rhs
    }
}

impl Add<BitCount> for usize {
    type Output = usize;

    fn add(self, rhs: BitCount) -> Self::Output {
        self + rhs.0
    }
}
