//! This module provides configuration types for [`crate::CuckooFilter`].

use std::{
    fmt::Display,
    num::{NonZeroU32, NonZeroUsize},
    ops::{Add, Deref, DerefMut},
};

use crate::data_block::DataBlockFieldConfiguration;

/// Error type for all configuration options.
#[derive(Debug)]
pub enum ConfigError {
    /// Error due to requesting buckets that are too big to represent (requiring over [`usize::MAX`]
    /// bytes).
    BucketTooBig,
    /// Error due to requesting more than 32 bits for any of the fields (fingerprint or associated
    /// field).
    BitCountTooHigh,
    /// Error due to requesting 0 bits for a field. If a field is enabled, it should take up at
    /// least 1 bit.
    BitCountTooLow,
}

impl Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::BucketTooBig => {
                f.write_str("Filter configuration requires buckets that are too big!")
            }
            ConfigError::BitCountTooHigh => f.write_str(&format!(
                "Bit count is too high! Max is {}.",
                BitCount::MAX.0
            )),
            ConfigError::BitCountTooLow => {
                f.write_str(&format!("Bit count too low! Min is {}.", BitCount::MIN.0))
            }
        }
    }
}

impl std::error::Error for ConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        None
    }
}

/// Builder for [`CuckooConfiguration`].
///
/// New instance can be created using [`CuckooConfiguration::builder`].
///
/// # Examples
///
/// ```
/// use cuckoo_clock::config::CuckooConfiguration;
/// let builder = CuckooConfiguration::builder(100_000);
/// ```
pub struct CuckooConfigurationBuilder {
    pub(crate) fingerprint_bits: BitCount,
    pub(crate) bucket_size: NonZeroUsize,
    pub(crate) max_entries: usize,
    pub(crate) max_kicks: usize,
    pub(crate) lru: Option<LruConfig>,
    pub(crate) ttl: Option<TtlConfig>,
    pub(crate) counter: Option<CounterConfig>,
}

impl CuckooConfigurationBuilder {
    /// Sets the number of bits used for fingerprint. Higher number of bits should result in less
    /// collisions, which should result in a lower false positive rate, at the cost of increased
    /// memory usage.
    #[must_use]
    pub const fn fingerprint_bits(mut self, bits: BitCount) -> Self {
        self.fingerprint_bits = bits;
        self
    }

    /// Sets the number of buckets to hold in a bucket. Larger buckets improve filter occupancy
    /// (space utilization), but they also require larger fingerprints to retain the same false
    /// positive rate.
    ///
    /// 5.1. Optimal bucket size in [the original paper] describes this relation.
    ///
    /// [the original paper]: https://www.cs.cmu.edu/~dga/papers/cuckoo-conext2014.pdf
    #[must_use]
    pub const fn bucket_size(mut self, size: NonZeroUsize) -> Self {
        self.bucket_size = size;
        self
    }

    /// Maximum number of kicks to perform if all requested slots are occupied when inserting new
    /// items. Items will be evicted and moved to their alternate slots until no more evictions are
    /// required or maximum number of kicks is reached.
    ///
    /// If the maximum number of kicks is reached, one item will be lost from the filter.
    ///
    /// Increasing this number will increase filter occupancy at the cost of insertion speed.
    #[must_use]
    pub const fn max_kicks(mut self, kicks: usize) -> Self {
        self.max_kicks = kicks;
        self
    }

    /// Enables LRU eviction for the filter. Kicks will no longer be performed randomly and will
    /// always target least recently used items, until either no more evictions are required, max
    /// number of kicks was reached or the kicked item is to be moved in a bucket with all slots
    /// occupied by more used items.
    ///
    /// When LRU is used, [`crate::CuckooFilter::scan_and_update_full`] should be called
    /// periodically, to age LRU for all items. It is up to the caller to schedule this process.
    /// More frequent scans will result in faster aging LRU for all items, requiring item to be
    /// used more frequently to outlive other items.
    #[must_use]
    pub const fn with_lru(mut self, lru: LruConfig) -> Self {
        self.lru = Some(lru);
        self
    }

    /// Enables TTL for items in the filter. TTL will be used to expire items from the filter when
    /// [`crate::CuckooFilter::scan_and_update_full`] is called.
    ///
    /// When TTL is used, [`crate::CuckooFilter::scan_and_update_full`] should be called
    /// periodically, to age TTL for all items. It is up to the caller to schedule this process.
    /// More frequent scans will result in lower TTL for all items.
    #[must_use]
    pub const fn with_ttl(mut self, ttl: TtlConfig) -> Self {
        self.ttl = Some(ttl);
        self
    }

    /// Enables counter for items in the filter. Counter is just provided as a value that can be
    /// read when accessing items. It is increased on every access (and can be controlled
    /// directly).
    #[must_use]
    pub const fn with_counter(mut self, counter: CounterConfig) -> Self {
        self.counter = Some(counter);
        self
    }

    /// Validates and builds the configuration.
    ///
    /// # Errors
    ///
    /// [`ConfigError::BucketTooBig`] if requests buckets are too big to represent with [`usize::MAX`].
    /// Bucket size is defined as [`Self::bucket_size`] * item bits (sum of all fields bits,
    /// rounded to byte).
    pub fn build(&self) -> Result<CuckooConfiguration, ConfigError> {
        let required_bucket_count = self.max_entries.div_ceil(self.bucket_size.get());
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
            bucket_size: self.bucket_size.get(),
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
                .get()
                .checked_mul(data_block_size)
                .ok_or(ConfigError::BucketTooBig)?,
            bucket_count,
            #[expect(clippy::cast_possible_truncation)]
            buckets_mask: (bucket_count - 1) as u32,
        })
    }
}

/// Configuration for the LRU field.
///
/// Used to define memory used by the LRU field, also affecting its maximum value.
///
/// # Examples
///
/// ```
/// use cuckoo_clock::config::LruConfig;
///
/// let ttl_config = LruConfig {
///     counter_bits: 5.try_into()?
/// };
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Clone, Debug)]
pub struct LruConfig {
    /// Number of bits used to represent the LRU counter.
    /// Larger bit counts allow more values to be represented, allowing items to "accumulate"
    /// higher use counts, which will take longer to age.
    pub counter_bits: BitCount,
}

impl Default for LruConfig {
    fn default() -> Self {
        Self {
            counter_bits: BitCount(8),
        }
    }
}

/// Configuration for the TTL field.
///
/// Used to define memory used by the TTL field and the default value.
///
/// # Examples
///
/// ```
/// use cuckoo_clock::config::TtlConfig;
///
/// let ttl_config = TtlConfig {
///     ttl: 100.try_into()?,
///     ttl_bits: 7.try_into()?
/// };
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Clone, Debug)]
pub struct TtlConfig {
    /// The default TTL counter value for newly inserted items. The actual lifetime duration will
    /// be defined by this value combined with the frequency of calls to
    /// [`crate::CuckooFilter::scan_and_update_full`]. Each call to
    /// [`crate::CuckooFilter::scan_and_update_full`] will reduce the counter by 1, until it
    /// reaches 0, when the item is removed.
    pub ttl: NonZeroU32,
    /// Number of bits used to represent the TTL counter.
    /// Larget bit counts allow higher TTL to be represented.
    pub ttl_bits: BitCount,
}

/// Configuration for the generic counter field.
///
/// Used to define memory used by the generic counter field, also affecting its maximum value.
///
/// # Examples
///
/// ```
/// use cuckoo_clock::config::CounterConfig;
///
/// let ttl_config = CounterConfig {
///     counter_bits: 5.try_into()?,
///     ..Default::default()
/// };
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Clone, Debug)]
pub struct CounterConfig {
    /// How many bits are used to represent the generic counter.
    /// Larget bit counts allow higher counter values to be represented.
    pub counter_bits: BitCount,
    /// Diff to apply to counter on each insert.
    pub change_on_insert: i32,
    /// Diff to apply to counter on each lookup.
    pub change_on_lookup: i32,
}

impl Default for CounterConfig {
    fn default() -> Self {
        Self {
            counter_bits: BitCount(4),
            change_on_insert: 1,
            change_on_lookup: 1,
        }
    }
}

/// Configuration for the [`crate::CuckooFilter`].
///
/// Used to define main cuckoo filter parameters (capacity, bucket size, fingeprint size,
/// max kicks), as well as additional features (TTL, LRU, generic counter).
///
/// Create a new instance using [`CuckooConfiguration::builder`].
///
/// # Examples
///
/// ```
/// use cuckoo_clock::config::CuckooConfiguration;
///
/// let config = CuckooConfiguration::builder(100_000)
///     .fingerprint_bits(14.try_into()?)
///     .bucket_size(4.try_into()?)
///     .max_kicks(8)
///     .build()?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
///
/// ```
/// use cuckoo_clock::config::{CuckooConfiguration, TtlConfig, LruConfig};
///
/// let config = CuckooConfiguration::builder(100_000)
///     .fingerprint_bits(14.try_into()?)
///     .with_ttl(TtlConfig {
///         ttl: 10.try_into()?,
///         ttl_bits: 4.try_into()?
///     })
///     .with_lru(LruConfig {
///         counter_bits: 6.try_into()?
///     })
///     .bucket_size(4.try_into()?)
///     .max_kicks(8)
///     .build()?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
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
    /// Creates a new instance of [`CuckooConfigurationBuilder`] with provided maximum number of
    /// entries.
    #[must_use]
    pub const fn builder(max_entries: usize) -> CuckooConfigurationBuilder {
        CuckooConfigurationBuilder {
            fingerprint_bits: BitCount(8),
            #[expect(clippy::expect_used)]
            bucket_size: NonZeroUsize::new(4).expect("4 != 0"),
            max_entries,
            max_kicks: 500,
            lru: None,
            ttl: None,
            counter: None,
        }
    }
}

/// Number of bits. Used to define sizes of the fields.
///
/// This value is limited by [`BitCount::MIN`] and [`BitCount::MAX`] and can only be created using
/// the [`TryFrom`] trait, to ensure the bit count is validated.
///
/// # Examples
///
/// ```
/// use cuckoo_clock::config::BitCount;
///
/// let bit_count: BitCount = 8.try_into().unwrap();
/// let bit_count_max: BitCount = 32.try_into().unwrap();
/// let bit_count_min: BitCount = 1.try_into().unwrap();
/// ```
///
/// ```should_panic
/// use cuckoo_clock::config::BitCount;
///
/// let bit_count: BitCount = 0.try_into().unwrap();
/// ```
///
/// ```should_panic
/// use cuckoo_clock::config::BitCount;
///
/// let bit_count: BitCount = 40.try_into().unwrap();
/// ```
#[derive(Debug, Clone, Copy)]
pub struct BitCount(usize);

impl BitCount {
    /// Maximum allowed value for [`BitCount`]
    pub const MAX: BitCount = BitCount(32);
    /// Minimum allowed value for [`BitCount`]
    pub const MIN: BitCount = BitCount(1);
}

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
    type Error = ConfigError;

    fn try_from(value: usize) -> Result<Self, Self::Error> {
        if value > Self::MAX.0 {
            return Err(ConfigError::BitCountTooHigh);
        }
        if value < Self::MIN.0 {
            return Err(ConfigError::BitCountTooLow);
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
    #[expect(clippy::cast_possible_truncation)]
    fn from(value: BitCount) -> Self {
        value.0 as u32
    }
}

impl From<BitCount> for u16 {
    #[expect(clippy::cast_possible_truncation)]
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
