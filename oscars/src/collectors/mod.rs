//! This module contains various collector implementations.

pub mod common;
pub mod mark_sweep;
pub mod mark_sweep_arena2;

#[cfg(feature = "mark_sweep_branded")]
pub mod mark_sweep_branded;
