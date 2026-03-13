//! Just your average garabage collector experimentation playground.
//!
//! Things that may be contained here in: allocators, garbage collectors, memory
//! management primitives.

#![cfg_attr(not(any(test, feature = "std")), no_std)]

extern crate self as oscars;

extern crate alloc as rust_alloc;

#[cfg(feature = "mark_sweep")]
pub use crate::collectors::mark_sweep::*;
#[cfg(feature = "mark_sweep")]
pub use oscars_derive::{Finalize, Trace};

#[cfg(feature = "mark_sweep")]
pub use crate::collectors::collector::Collector;

#[cfg(feature = "gc_allocator")]
pub use crate::collectors::mark_sweep::{GcAllocBox, GcAllocVec};

pub mod alloc;
pub mod collectors;
