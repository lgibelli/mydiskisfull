//! mdif-core: honest disk accounting for macOS.
//!
//! - [`scan`]: parallel tree walk that measures allocated, private (freed if
//!   deleted), clone/snapshot-shared and cloud-only bytes.
//! - [`apfs`]: containers, volumes and local Time Machine snapshots.
//! - [`map`]: the whole-disk map, reconciled against what APFS reports.
//! - [`watch`]: the sampler; [`store`]: its decimated history; [`forecast`]:
//!   when the disk will be full.
//! - [`owners`]: which folder space belongs to, and how fast each grows.
//! - [`privacy`]: the folders a background process mustn't open without Full
//!   Disk Access, because macOS would wait on a prompt.
//! - [`incidents`]: runaway fillers (growth, churn), in one sentence.
//! - [`tweaks`]: reversible settings that stop something from coming back,
//!   built on [`settings`] and [`profile`]; [`suggest`]: what to do about
//!   each cause.

pub mod apfs;
pub mod forecast;
pub mod incidents;
pub mod map;
pub mod owners;
pub mod privacy;
pub mod profile;
pub mod scan;
pub mod settings;
pub mod store;
pub mod suggest;
pub mod text;
pub mod tweaks;
pub mod units;
pub mod watch;
