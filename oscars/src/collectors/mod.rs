//! This module contains various collector implementations.

pub mod common;
#[cfg(feature = "mark_sweep")]
pub mod mark_sweep;
#[cfg(feature = "mark_sweep2")]
pub mod mark_sweep_arena2;
#[cfg(feature = "null_collector")]
pub mod null_collector;

// TODO: Implement a null collector for the branded API as well
#[cfg(feature = "mark_sweep_branded")]
pub mod mark_sweep_branded;
