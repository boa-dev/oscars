//! Just your average garabage collector experimentation playground.
//!
//! Things that may be contained here in: allocators, garbage collectors, memory
//! management primitives.

#![no_std]

extern crate self as oscars;

extern crate alloc as rust_alloc;

#[cfg(feature = "std")]
extern crate std;

#[cfg(feature = "mark_sweep")]
pub use crate::collectors::mark_sweep::*;
#[cfg(feature = "mark_sweep")]
pub use oscars_derive::{Finalize, Trace};

#[cfg(feature = "mark_sweep")]
pub mod collector;
#[cfg(feature = "mark_sweep")]
pub use collector::{Collector, GcAllocator};

pub mod alloc;
pub mod collectors;
