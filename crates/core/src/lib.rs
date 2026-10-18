//! Clipier without a window. See `ARCHITECTURE.md` for what lives here and what does not.

pub mod agents;
pub mod check;
pub mod config;
pub mod crew;
pub mod git;
#[cfg(feature = "github")]
pub mod github;
pub mod id;
pub mod memory;
pub mod plan;
pub mod platform;
pub mod race;
pub mod roles;
pub mod runner;
pub mod scope;
pub mod session;
pub mod store;
pub mod usage;
