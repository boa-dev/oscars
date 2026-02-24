//! This module provides a variety of experimental allocators written in Rust

pub mod arena;
pub mod arena2;
pub mod mempool;
pub mod mempool2;

#[cfg(feature = "gc_allocator")]
pub mod gc_allocator;
