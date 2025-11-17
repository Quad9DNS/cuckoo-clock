use std::fmt::Display;

#[derive(Debug)]
pub enum Error {
    BucketTooBig,
    BitCountTooHigh,
}

impl Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::BucketTooBig => {
                f.write_str("Filter configuration requires buckets that are too big!")
            }
            Error::BitCountTooHigh => f.write_str("Bit count is too high! Max is 32."),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        None
    }
}
