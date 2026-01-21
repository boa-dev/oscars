//! Just your average garabage collector experimentation playground.
//!
//! Things that may be contained here in: allocators, garbage collectors, memory
//! management primitives.

#![no_std]

extern crate alloc as rust_alloc;

//#[cfg(feature = "std")]
extern crate std;

pub mod alloc;
pub mod collectors;
