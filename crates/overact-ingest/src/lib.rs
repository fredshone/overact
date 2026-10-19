//! overact-ingest: read an activities table and an optional attributes table
//! (CSV or Parquet) straight into an [`overact_core::Dataset`].
//!
//! This is the one implementation of the packing rules (cleaning, fix
//! counts, person order, activity order, column encodings). The
//! `overact-pack` binary writes its result to an `.overact` file (`uv run
//! overact-pack` runs that binary); the browser loads tables through it
//! directly. Pure Rust: the browser build supplies a [`ByteSource`].

pub mod attrs;
pub mod clean;
pub mod ids;
pub mod ingest;
pub mod inspect;
pub mod source;
pub mod table;

pub use ingest::{ingest, ingest_with_report, Config, TimeUnit};
pub use inspect::inspect;
pub use source::{ByteSource, FileSource};
pub use table::Table;

use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub struct Error(pub String);

impl Error {
    pub fn new(msg: impl Into<String>) -> Error {
        Error(msg.into())
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Error {
        Error(format!("read error: {e}"))
    }
}

impl From<parquet::errors::ParquetError> for Error {
    fn from(e: parquet::errors::ParquetError) -> Error {
        Error(format!("Parquet: {e}"))
    }
}

impl From<overact_core::Error> for Error {
    fn from(e: overact_core::Error) -> Error {
        Error(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// Progress callback: `(phase, done, total)`.
pub type Progress<'a> = &'a mut dyn FnMut(&str, u64, u64);
