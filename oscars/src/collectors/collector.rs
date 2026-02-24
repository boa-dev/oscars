// exploring `trait Collector: Allocator`
//
// the limitations:
//-  since rust won't let us cfg-gate a supertrait bound 
//    directly, so we define `Collector` twice
//- the borrow wall: `Allocator` uses `&self` but `collect()` needs `&mut self`. 
//    if a collection is live, calling `collect(&mut self)` fails to compile
// - fix: change to `collect(&self)` and put the gc state in a `RefCell`
// - if allocating triggers a gc cycle, we double borrow 
//    the arena and panic. we need to separate allocation from collection triggers
// - `Allocator` isn't object-safe, so `dyn Collector` won't work


// when `gc_allocator` is on, third-party collections can use the gc's arena directly
//
// warning:
//
// this compiles, but triggering a collection while holding an allocation
// causes a borrow conflict
#[cfg(feature = "gc_allocator")]
pub trait Collector: allocator_api2::alloc::Allocator {
    // trigger a full collection cycle
    fn collect(&mut self);
}

//used when `gc_allocator` is off
#[cfg(not(feature = "gc_allocator"))]
pub trait Collector {
    fn collect(&mut self);
}
