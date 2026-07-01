use std::{error::Error, fmt::Display, num::TryFromIntError, sync::Mutex};
#[allow(deprecated)]
use std::{
    hash::{BuildHasher, SipHasher},
    io::{Read, Write},
};

use crate::{
    bucket::Bucket,
    config::{
        BitCount, CounterConfig, CuckooConfiguration, LruAgingStrategy, LruConfig, TtlConfig,
    },
};

/// Error type for import operations.
#[derive(Debug)]
pub enum ImportError {
    /// Error due to underlying IO error.
    Io(std::io::Error),
    /// Error due to reading invalid stored configuration.
    Config(crate::config::ConfigError),
    /// Error due to reading a zero value for a field that does not allow zeroes.
    NonZeroError(TryFromIntError),
    /// Error due to mismatch in hasher names, between the stored and expected hasher.
    InvalidHasherName {
        /// Expected name for the hasher (defined by the type of hasher for [`crate::CuckooFilter`]).
        expected: String,
        /// Found name for the hasher (value stored in the exported data).
        found: String,
    },
    /// Error due to malformed header in the export.
    InvalidHasherHeader {
        /// Found value in the header that did not match the standard value.
        found: String,
    },
}

/// Error type for export operations.
#[derive(Debug)]
pub enum ExportError {
    /// Error due to underlying IO error.
    Io(std::io::Error),
}

impl Display for ImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ImportError::Io(error) => write!(f, "IO error while importing: {}", error),
            ImportError::Config(error) => {
                write!(f, "config error on imported data: {}", error)
            }
            ImportError::NonZeroError(error) => {
                write!(f, "conversion error in imported data: {}", error)
            }
            ImportError::InvalidHasherName { expected, found } => write!(
                f,
                "Invalid hasher name found in header: \"{}\". Expected: \"{}\"",
                found, expected
            ),
            ImportError::InvalidHasherHeader { found } => write!(
                f,
                "Invalid hasher header. Expected \"{}\" at the end, but found \"{}\"",
                HEADER_END, found
            ),
        }
    }
}

impl Error for ImportError {
    fn cause(&self) -> Option<&dyn Error> {
        match self {
            ImportError::Io(error) => Some(error),
            ImportError::Config(error) => Some(error),
            ImportError::NonZeroError(error) => Some(error),
            ImportError::InvalidHasherName { .. } => None,
            ImportError::InvalidHasherHeader { .. } => None,
        }
    }
}

impl From<std::io::Error> for ImportError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<crate::config::ConfigError> for ImportError {
    fn from(value: crate::config::ConfigError) -> Self {
        Self::Config(value)
    }
}

impl From<TryFromIntError> for ImportError {
    fn from(value: TryFromIntError) -> Self {
        Self::NonZeroError(value)
    }
}

impl Display for ExportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExportError::Io(error) => write!(f, "IO error while exporting: {}", error),
        }
    }
}

impl Error for ExportError {
    fn cause(&self) -> Option<&dyn Error> {
        match self {
            ExportError::Io(error) => Some(error),
        }
    }
}

impl From<std::io::Error> for ExportError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

const HEADER: &str = "cuckoo-clock:";
const HEADER_END: &str = ":header-end\n";

/// A trait representing objects that can be exported (e.g. persisted into a file).
///
/// This trait is needed for [`cuckoo_clock::CuckooFilter::export`].
pub trait Exportable: Clone {
    /// Writes the state of this object into the provided writer.
    fn write_to(&self, writer: impl Write) -> Result<(), ExportError>;

    /// Reads the stored state to create an instance of this object.
    fn read_from(reader: impl Read) -> Result<Self, ImportError>;
}

/// A trait representing [`BuildHasher`] instances that can be exported (e.g. persisted into a file).
///
/// This trait is needed for [`cuckoo_clock::CuckooFilter::export`].
pub trait ExportableBuildHasher: Exportable + BuildHasher {
    /// Unique of this [`ExportableBuildHasher`]. This will be stored in the header of exported
    /// data.
    const NAME: &str;
}

/// Implementation of [`ExportableRandomState`], similar to [`std::hash::RandomState`].
#[derive(Clone)]
pub struct ExportableRandomState {
    k0: u64,
    k1: u64,
}

impl ExportableRandomState {
    pub(crate) const fn new() -> Self {
        Self { k0: 0, k1: 0 }
    }

    /// Creates a new random instance.
    #[must_use]
    pub fn new_random() -> Self {
        Self {
            k0: rand::random(),
            k1: rand::random(),
        }
    }
}

impl Default for ExportableRandomState {
    fn default() -> Self {
        Self::new()
    }
}

#[allow(deprecated)]
impl BuildHasher for ExportableRandomState {
    type Hasher = SipHasher;

    fn build_hasher(&self) -> Self::Hasher {
        SipHasher::new_with_keys(self.k0, self.k1)
    }
}

impl Exportable for ExportableRandomState {
    fn write_to(&self, mut writer: impl Write) -> Result<(), ExportError> {
        let k0 = self.k0.to_be_bytes();
        let k1 = self.k1.to_be_bytes();
        writer.write_all(&k0)?;
        writer.write_all(&k1)?;
        Ok(())
    }

    fn read_from(mut reader: impl Read) -> Result<Self, ImportError> {
        let mut buf = [0u8; 8];
        reader.read_exact(&mut buf)?;
        let k0 = u64::from_be_bytes(buf);
        reader.read_exact(&mut buf)?;
        let k1 = u64::from_be_bytes(buf);
        Ok(Self { k0, k1 })
    }
}

impl ExportableBuildHasher for ExportableRandomState {
    const NAME: &str = "cuckoo_clock::exporter::ExportableRandomState";
}

/// Exporter for [`crate::CuckooFilter`].
///
/// This implements [`Read`] to allow reading this data into whatever storage required.
pub struct CuckooFilterExporter<'a, H: ExportableBuildHasher> {
    hasher: &'a H,
    buckets: &'a Vec<Mutex<Bucket>>,
    config: &'a CuckooConfiguration,
}

impl<'a, H: ExportableBuildHasher> CuckooFilterExporter<'a, H> {
    pub(crate) const fn new(
        hasher: &'a H,
        buckets: &'a Vec<Mutex<Bucket>>,
        config: &'a CuckooConfiguration,
    ) -> Self {
        Self {
            hasher,
            buckets,
            config,
        }
    }

    /// Reads in the entire [`crate::CuckooFilter`] state into the provided [`Write`] instance.
    ///
    /// This will lock buckets one by one and can be run concurrently with other
    /// [`crate::CuckooFilter`] operations, but it may result in a state which
    /// combines old values for some buckets and newer valus for some other buckets.
    /// The resulting state should still be valid.
    pub fn write_to(&self, mut writer: impl Write) -> Result<(), ExportError> {
        writer.write_all(format!("{}:{}:", HEADER, H::NAME).as_bytes())?;
        self.hasher.write_to(&mut writer)?;
        writer.write_all(HEADER_END.as_bytes())?;

        self.config.write_to(&mut writer)?;
        writer.write_all(b"\n")?;

        for b in self.buckets.iter() {
            #[expect(clippy::unwrap_used)]
            let bucket = b.lock().unwrap();
            bucket.export(&mut writer)?;
        }

        Ok(())
    }

    /// Reads the entire [`crate::CuckooFilter`] state into a [`Vec`].
    ///
    /// This can be useful if writing to slower [`Write`] interfaces, to ensure less changes are
    /// made while exporting is in progress.
    pub fn snapshot(&self) -> Result<Vec<u8>, ExportError> {
        let mut result = Vec::new();
        self.write_to(&mut result)?;
        Ok(result)
    }
}

pub(crate) fn read_hasher_from<H: ExportableBuildHasher + BuildHasher>(
    mut reader: impl Read,
) -> Result<H, ImportError> {
    let mut header_prefix = vec![0; HEADER.len() + H::NAME.len() + 2];
    reader.read_exact(&mut header_prefix)?;
    if header_prefix != format!("{}:{}:", HEADER, H::NAME).as_bytes() {
        return Err(ImportError::InvalidHasherName {
            expected: H::NAME.to_string(),
            found: String::from_utf8(header_prefix)
                .map(|s| {
                    s.trim_start_matches(&format!("{}:", HEADER))
                        .trim_end_matches(":")
                        .to_string()
                })
                .unwrap_or("<non UTF-8 data>".to_string()),
        });
    };
    let hasher = H::read_from(&mut reader)?;
    let mut header_suffix = vec![0; HEADER_END.len()];
    reader.read_exact(&mut header_suffix)?;
    if header_suffix != HEADER_END.as_bytes() {
        return Err(ImportError::InvalidHasherHeader {
            found: String::from_utf8(header_suffix).unwrap_or("<non UTF-8 data>".to_string()),
        });
    };
    Ok(hasher)
}

impl Exportable for CuckooConfiguration {
    fn write_to(&self, mut writer: impl std::io::Write) -> Result<(), crate::ExportError> {
        // 0 as version start, and the rest is lib version
        writer.write_all(&[0, 0, 2, 4])?;
        // Since we are reading a value from out field config, we know that it can't ever be higher
        // than 32 and should fit into u8.
        #[allow(clippy::expect_used)]
        // Value mask is calculated as bits ^ 2 - 1, so we get back the bit count with ilog2
        writer.write_all(
            &u8::try_from((self.fingerprint_field_config.value_mask() as usize + 1).ilog2())
                .expect("Fingeprint bits can't be higher than 32")
                .to_be_bytes(),
        )?;
        writer.write_all(&(self.bucket_size as u64).to_be_bytes())?;
        // Max entries
        writer.write_all(&((self.bucket_count * self.bucket_size) as u64).to_be_bytes())?;
        writer.write_all(&(self.max_kicks as u64).to_be_bytes())?;
        if let Some((lru, _)) = &self.lru_field_config {
            writer.write_all(&[1])?;
            lru.write_to(&mut writer)?;
        }
        if let Some((ttl, _)) = &self.ttl_field_config {
            writer.write_all(&[2])?;
            ttl.write_to(&mut writer)?;
        }
        if let Some((counter, _)) = &self.counter_field_config {
            writer.write_all(&[3])?;
            counter.write_to(&mut writer)?;
        }
        Ok(())
    }

    fn read_from(mut reader: impl std::io::Read) -> Result<Self, crate::ImportError> {
        let mut u8_buf = [0u8; 1];
        let mut u64_buf = [0u8; 8];
        reader.read_exact(&mut u8_buf)?;
        let mut version: usize = 0;
        if u8::from_be_bytes(u8_buf) == 0 {
            // This is a version header
            reader.read_exact(&mut u8_buf)?;
            version += 1_000_000 * usize::from(u8::from_be_bytes(u8_buf));
            reader.read_exact(&mut u8_buf)?;
            version += 1_000 * usize::from(u8::from_be_bytes(u8_buf));
            reader.read_exact(&mut u8_buf)?;
            version += usize::from(u8::from_be_bytes(u8_buf));
            reader.read_exact(&mut u8_buf)?;
        } else {
            // This is an old version (< 0.2.0) - without version header
            version = 1_000;
        }
        let fp_bits: BitCount = usize::from(u8::from_be_bytes(u8_buf)).try_into()?;
        reader.read_exact(&mut u64_buf)?;
        let bucket_size = u64::from_be_bytes(u64_buf);
        reader.read_exact(&mut u64_buf)?;
        let max_entries = u64::from_be_bytes(u64_buf);
        reader.read_exact(&mut u64_buf)?;
        let max_kicks = u64::from_be_bytes(u64_buf);
        let mut builder = CuckooConfiguration::builder(max_entries.try_into()?)
            .fingerprint_bits(fp_bits)
            .bucket_size(usize::try_from(bucket_size)?.try_into()?)
            .max_kicks(max_kicks.try_into()?);
        while let Ok(()) = reader.read_exact(&mut u8_buf) {
            let conf_type = u8::from_be_bytes(u8_buf);
            match conf_type {
                1 => {
                    builder = builder.with_lru(LruConfig::read_from(&mut reader, version)?);
                }
                2 => {
                    builder = builder.with_ttl(TtlConfig::read_from(&mut reader, version)?);
                }
                3 => {
                    builder = builder.with_counter(CounterConfig::read_from(&mut reader, version)?);
                }
                // We can't handle this type, so abort
                // TODO: return an error?
                _ => break,
            }
        }
        Ok(builder.build()?)
    }
}

/// A trait representing config objects that can be exported (e.g. persisted into a file).
pub(crate) trait ExportableConfig: Clone {
    /// Writes the state of this object into the provided writer.
    fn write_to(&self, writer: impl Write) -> Result<(), ExportError>;

    /// Reads the stored state to create an instance of this object.
    fn read_from(reader: impl Read, version: usize) -> Result<Self, ImportError>;
}

impl ExportableConfig for LruConfig {
    fn write_to(&self, mut writer: impl Write) -> Result<(), ExportError> {
        writer.write_all(&u8::from(self.counter_bits).to_be_bytes())?;
        writer.write_all(&u8::from(self.remove_on_zero).to_be_bytes())?;
        self.aging_strategy.write_to(&mut writer)?;
        writer.write_all(&self.starting_value.to_be_bytes())?;
        writer.write_all(&self.increment.to_be_bytes())?;
        Ok(())
    }

    fn read_from(mut reader: impl Read, version: usize) -> Result<Self, ImportError> {
        let mut u8_buf = [0u8; 1];
        let mut u32_buf = [0u8; 4];
        reader.read_exact(&mut u8_buf)?;
        let bits = u8::from_be_bytes(u8_buf);
        let mut remove_on_zero = false;
        let mut aging_strategy = LruAgingStrategy::default();
        let mut increment = 1;
        let mut starting_value = 1;
        if version >= 2_000 {
            reader.read_exact(&mut u8_buf)?;
            remove_on_zero = u8::from_be_bytes(u8_buf) != 0;
        }
        if version >= 2_002 {
            aging_strategy = LruAgingStrategy::read_from(&mut reader, version)?;
            reader.read_exact(&mut u32_buf)?;
            starting_value = u32::from_be_bytes(u32_buf);
            reader.read_exact(&mut u32_buf)?;
            increment = u32::from_be_bytes(u32_buf);
        }
        Ok(Self {
            counter_bits: (bits as usize).try_into()?,
            aging_strategy,
            starting_value,
            remove_on_zero,
            increment,
        })
    }
}

impl ExportableConfig for LruAgingStrategy {
    fn write_to(&self, mut writer: impl Write) -> Result<(), ExportError> {
        match self {
            LruAgingStrategy::Halving => writer.write_all(&1u8.to_be_bytes())?,
            LruAgingStrategy::Decrement(value) => {
                writer.write_all(&2u8.to_be_bytes())?;
                writer.write_all(&value.to_be_bytes())?;
            }
        }
        Ok(())
    }

    fn read_from(mut reader: impl Read, _version: usize) -> Result<Self, ImportError> {
        let mut u8_buf = [0u8; 1];
        let mut u32_buf = [0u8; 4];
        reader.read_exact(&mut u8_buf)?;
        let strategy = u8::from_be_bytes(u8_buf);
        match strategy {
            1 => Ok(Self::Halving),
            2 => {
                reader.read_exact(&mut u32_buf)?;
                let value = u32::from_be_bytes(u32_buf);
                Ok(Self::Decrement(value))
            }
            // TODO: error for this?
            _ => Ok(Self::default()),
        }
    }
}

impl ExportableConfig for TtlConfig {
    fn write_to(&self, mut writer: impl Write) -> Result<(), ExportError> {
        writer.write_all(&self.ttl.get().to_be_bytes())?;
        writer.write_all(&u8::from(self.ttl_bits).to_be_bytes())?;
        Ok(())
    }

    fn read_from(mut reader: impl Read, _version: usize) -> Result<Self, ImportError> {
        let mut u8_buf = [0u8; 1];
        let mut u32_buf = [0u8; 4];
        reader.read_exact(&mut u32_buf)?;
        let ttl = u32::from_be_bytes(u32_buf);
        reader.read_exact(&mut u8_buf)?;
        let bits = u8::from_be_bytes(u8_buf);
        Ok(Self {
            ttl: ttl.try_into()?,
            ttl_bits: (bits as usize).try_into()?,
        })
    }
}

impl ExportableConfig for CounterConfig {
    fn write_to(&self, mut writer: impl Write) -> Result<(), ExportError> {
        writer.write_all(&u8::from(self.counter_bits).to_be_bytes())?;
        writer.write_all(&self.change_on_insert.to_be_bytes())?;
        writer.write_all(&self.change_on_lookup.to_be_bytes())?;
        Ok(())
    }

    fn read_from(mut reader: impl Read, _version: usize) -> Result<Self, ImportError> {
        let mut u8_buf = [0u8; 1];
        let mut u32_buf = [0u8; 4];
        reader.read_exact(&mut u8_buf)?;
        let bits = u8::from_be_bytes(u8_buf);
        reader.read_exact(&mut u32_buf)?;
        let change_on_insert = i32::from_be_bytes(u32_buf);
        reader.read_exact(&mut u32_buf)?;
        let change_on_lookup = i32::from_be_bytes(u32_buf);
        Ok(Self {
            counter_bits: (bits as usize).try_into()?,
            change_on_insert,
            change_on_lookup,
        })
    }
}
