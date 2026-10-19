//! overact-core: a small columnar engine for human activity schedules.
//!
//! This crate has no WASM or DOM dependencies so it can also back a native
//! server. The browser build lives in `overact-wasm`.

pub mod column;
pub mod dataset;
pub mod engine;
pub mod export;
pub mod features;
pub mod filter;
pub mod format;
pub mod profile;
pub mod raster;
pub mod sample;
pub mod sort;

#[cfg(test)]
mod tests_engine;
#[cfg(all(test, feature = "parallel"))]
mod tests_parallel;
#[cfg(test)]
pub(crate) mod testutil;

pub use column::{Column, ColumnData};
pub use dataset::{Dataset, Part};
pub use engine::{Engine, ViewSpec};
pub use format::{Error, Header, Result};
