//! mdif-core: honest disk accounting for macOS.
//!
//! - [`scan`]: parallel tree walk that measures allocated, private (freed if
//!   deleted), clone/snapshot-shared and cloud-only bytes.
//! - [`apfs`]: containers, volumes and local Time Machine snapshots.
//! - [`map`]: the whole-disk map, reconciled against what APFS reports.
//! - [`watch`]: the sampler; [`store`]: its decimated history; [`forecast`]:
//!   when the disk will be full.
//! - [`owners`]: which folder space belongs to, and how fast each grows.
//! - [`clean`]: the clean-ups it does itself when asked, checked again first;
//!   [`tidy`]: the ones the watcher does once a day, when turned on.
//! - [`keep`]: what you keep, which nothing suggests deleting.
//! - [`space`]: where the space goes, sorted into kinds, and what of it is
//!   waste.
//! - [`worktrees`]: agent worktrees whose work is merged, with nothing left
//!   in them.
//! - [`privacy`]: the folders a background process mustn't open without Full
//!   Disk Access, because macOS would wait on a prompt.
//! - [`incidents`]: runaway fillers (growth, churn), in one sentence;
//!   [`alerts`]: the thresholds for them and for the forecast that the user
//!   may set.
//! - [`tweaks`]: reversible settings that stop something from coming back,
//!   built on [`settings`] and [`profile`]; [`suggest`]: what to do about
//!   each cause.

pub mod alerts;
pub mod apfs;
pub mod clean;
pub mod docker;
pub mod forecast;
pub mod handlers;
pub mod incidents;
pub mod keep;
pub mod map;
pub mod os_update;
pub mod owners;
pub mod privacy;
pub mod profile;
pub mod scan;
pub mod settings;
pub mod space;
pub mod store;
pub mod suggest;
pub mod text;
pub mod tidy;
pub mod timemachine;
pub mod tweaks;
pub mod units;
pub mod watch;
pub mod worktrees;
