//! mdif-core: honest disk accounting for macOS.
//!
//! - [`scan`]: parallel tree walk that measures allocated, private (freed if
//!   deleted), clone/snapshot-shared and cloud-only bytes.
//! - [`apfs`]: containers, volumes and local Time Machine snapshots.
//! - [`map`]: the whole-disk map, reconciled against what APFS reports.
//! - [`watch`]: the sampler; [`store`]: its decimated history; [`forecast`]:
//!   when the disk will be full.

pub mod apfs;
pub mod forecast;
pub mod map;
pub mod scan;
pub mod store;
pub mod text;
pub mod units;
pub mod watch;
