//! Pointers represents the External types returned by the Boa Garbage Collector

mod gc;
mod weak;
pub(crate) mod weak_map;

pub use gc::{Gc, Root};
pub use weak::WeakGc;
pub use weak_map::WeakMap;
