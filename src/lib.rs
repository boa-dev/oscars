//! Just your average garabage collector experimentation playground.
//!
//! Things that may be contained here in: allocators, garbage collectors, memory
//! management primitives.

#![no_std]

extern crate alloc as rust_alloc;

pub mod arena;
pub mod arena2;
pub mod mempool;
pub mod mempool2;
