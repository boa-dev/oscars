//! Pointers represents the External types returned by the Boa Garbage Collector

mod gc;
mod weak;
mod weak_map;

pub use gc::Gc;
pub use weak::WeakGc;
