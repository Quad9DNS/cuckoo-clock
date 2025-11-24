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
#![deny(clippy::unchecked_duration_subtraction)]
#![warn(clippy::unwrap_used)]
#![warn(clippy::expect_used)]
#![deny(clippy::panicking_unwrap)]
#![deny(clippy::option_env_unwrap)]
#![deny(clippy::uninit_vec)]
#![deny(unnecessary_transmutes)]
#![deny(clippy::transmute_ptr_to_ref)]
#![deny(clippy::transmute_undefined_repr)]
#![deny(clippy::missing_const_for_fn)]

pub mod associated_data;
mod bucket;
pub mod config;
mod data_block;
mod error;
pub mod filter;

pub use error::Error;
pub type Result<T> = core::result::Result<T, Error>;
pub use {config::CuckooConfiguration, data_block::Fingerprint, filter::CuckooFilter};
