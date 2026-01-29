//! This crate provides a thread-safe, extended version of [Cuckoo filter], supporting TTL, LRU and
//! custom counters associated with stored data.
//!
//! All the limitations of original Cuckoo filter still hold, meaning that all additional data is
//! associated with the fingerprint, which may represent a completely different item. For that reason,
//! it is recommended to only use associated data to manage the lifetime of the items, unless false
//! positives are not problematic in your use-case.
//!
//! This implementation can be used as a regular Cuckoo filter, without the added features too.
//!
//! [Cuckoo filter]: https://www.cs.cmu.edu/~binfan/papers/conext14_cuckoofilter.pdf

#![deny(clippy::fallible_impl_from)]
#![deny(clippy::wildcard_enum_match_arm)]
#![deny(clippy::unneeded_field_pattern)]
#![deny(clippy::fn_params_excessive_bools)]
#![deny(clippy::must_use_candidate)]
#![deny(arithmetic_overflow)]
#![deny(clippy::checked_conversions)]
#![deny(clippy::cast_possible_truncation)]
#![deny(clippy::cast_sign_loss)]
#![deny(clippy::cast_possible_wrap)]
#![deny(clippy::cast_precision_loss)]
#![deny(clippy::unchecked_time_subtraction)]
#![warn(clippy::unwrap_used)]
#![warn(clippy::expect_used)]
#![deny(clippy::panicking_unwrap)]
#![deny(clippy::option_env_unwrap)]
#![deny(clippy::uninit_vec)]
#![deny(unnecessary_transmutes)]
#![deny(clippy::transmute_ptr_to_ref)]
#![deny(clippy::transmute_undefined_repr)]
#![deny(clippy::missing_const_for_fn)]
#![warn(missing_docs)]

mod associated_data;
mod bucket;
pub mod config;
mod data_block;
mod exporter;
mod filter;

pub use {
    associated_data::AssociatedData, bucket::InsertValues, bucket::LookupValues,
    data_block::Fingerprint, exporter::CuckooFilterExporter, exporter::ExportError,
    exporter::ExportableBuildHasher, exporter::ExportableRandomState, exporter::ImportError,
    filter::CuckooFilter,
};
