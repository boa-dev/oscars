use hashbrown::HashTable;
use rustc_hash::FxHasher;

use crate::{
    alloc::mempool3::PoolPointer,
    collectors::collector::Collector,
    collectors::mark_sweep::{Finalize, TraceColor, internals::Ephemeron, trace::Trace},
};
use core::{hash::Hasher, ptr::NonNull};

use super::Gc;

#[inline]
fn hash_addr(addr: usize) -> u64 {
    let mut h = FxHasher::default();
    h.write_usize(addr);
    h.finish()
}

// type erased trait so the collector can prune any WeakMap without knowing K/V
#[doc(hidden)]
pub trait ErasedWeakMap {
    fn prune_dead_entries(&mut self, color: TraceColor);
    fn is_alive(&self) -> bool;
}

// the actual weak map store, managed by the collector
struct WeakMapInner<K: Trace + 'static, V: Trace + 'static> {
    // keyed by the raw pointer address of the GC object; stored inline as
    // `(addr, ptr)` so HashTable needs no separate key allocation
    entries: HashTable<(usize, PoolPointer<'static, Ephemeron<K, V>>)>,
    is_alive: core::cell::Cell<bool>,
}

impl<K: Trace, V: Trace> WeakMapInner<K, V> {
    fn new() -> Self {
        Self {
            entries: HashTable::new(),
            is_alive: core::cell::Cell::new(true),
        }
    }

    fn remove_and_invalidate(&mut self, key_addr: usize) {
        if let Ok(entry) = self
            .entries
            .find_entry(hash_addr(key_addr), |e| e.0 == key_addr)
        {
            let ((_, old_ephemeron), _) = entry.remove();
            old_ephemeron.as_inner_ref().invalidate();
        }
    }

    fn insert_ptr(
        &mut self,
        key_addr: usize,
        ephemeron_ptr: PoolPointer<'static, Ephemeron<K, V>>,
    ) {
        // caller guarantees no duplicate exists since remove_and_invalidate was called first
        self.entries
            .insert_unique(hash_addr(key_addr), (key_addr, ephemeron_ptr), |e| {
                hash_addr(e.0)
            });
    }

    fn get(&self, key: &Gc<K>) -> Option<&V> {
        let key_addr = key.inner_ptr.as_non_null().as_ptr() as usize;
        self.entries
            .find(hash_addr(key_addr), |e| e.0 == key_addr)
            .map(|(_, p)| p.as_inner_ref().value())
    }

    fn is_key_alive(&self, key: &Gc<K>) -> bool {
        let key_addr = key.inner_ptr.as_non_null().as_ptr() as usize;
        self.entries
            .find(hash_addr(key_addr), |e| e.0 == key_addr)
            .is_some()
    }

    fn remove(&mut self, key: &Gc<K>) -> bool {
        let key_addr = key.inner_ptr.as_non_null().as_ptr() as usize;
        // the backing ephemeron stays in the collector queue and gets swept
        // when the key is collected
        if let Ok(entry) = self
            .entries
            .find_entry(hash_addr(key_addr), |e| e.0 == key_addr)
        {
            let ((_, ptr), _) = entry.remove();
            ptr.as_inner_ref().invalidate();
            true
        } else {
            false
        }
    }
}

impl<K: Trace, V: Trace> ErasedWeakMap for WeakMapInner<K, V> {
    fn prune_dead_entries(&mut self, color: TraceColor) {
        self.entries
            .retain(|(_, ephemeron_ptr)| ephemeron_ptr.as_inner_ref().is_reachable(color));
    }

    fn is_alive(&self) -> bool {
        self.is_alive.get()
    }
}

// map that prunes entries automatically when their GC keys are collected
//
// the collector owns the `WeakMapInner` heap allocation, `WeakMap` holds a
// raw pointer back to it
//
// single threaded: the GC and all `WeakMap` ops run on the same thread
//  lifetime ordering: `WeakMap` must not outlive its collector
// no aliased writes: collector only mutates through box during `collect()`
pub struct WeakMap<K: Trace + 'static, V: Trace + 'static> {
    // raw pointer to collector owned memory
    inner: NonNull<WeakMapInner<K, V>>,
}

impl<K: Trace, V: Trace> WeakMap<K, V> {
    // create a new map and give the collector ownership of its memory
    pub fn new<C: Collector>(collector: &C) -> Self {
        let boxed: rust_alloc::boxed::Box<WeakMapInner<K, V>> =
            rust_alloc::boxed::Box::new(WeakMapInner::<K, V>::new());

        // turn into a raw pointer so the collector can share it safely
        let inner_ptr: *mut WeakMapInner<K, V> = rust_alloc::boxed::Box::into_raw(boxed);
        // SAFETY: pointer returned from `Box::into_raw` is non-null
        let inner = unsafe { NonNull::new_unchecked(inner_ptr) };

        collector.track_weak_map(inner);
        Self { inner }
    }

    pub fn insert<C: Collector>(&mut self, key: &Gc<K>, value: V, collector: &C) {
        let key_addr = key.inner_ptr.as_non_null().as_ptr() as usize;

        // remove and invalidate any existing ephemeron for this key
        // SAFETY: we have unique access to `self`
        unsafe { self.inner.as_mut().remove_and_invalidate(key_addr) };

        //allocate the new ephemeron node
        let ephemeron_ptr = collector
            .alloc_ephemeron_node(key, value)
            .expect("Failed to allocate ephemeron");

        // SAFETY: safe because the gc tracks this
        let ephemeron_ptr = unsafe { ephemeron_ptr.extend_lifetime() };

        //insert the new node using another short lived mutable borrow
        // SAFETY: we have unique access to `self`
        unsafe { self.inner.as_mut().insert_ptr(key_addr, ephemeron_ptr) };
    }

    pub fn get(&self, key: &Gc<K>) -> Option<&V> {
        // SAFETY: we hold `&self` so the map is alive and unchanged
        unsafe { self.inner.as_ref().get(key) }
    }

    pub fn is_key_alive(&self, key: &Gc<K>) -> bool {
        // SAFETY: we hold `&self` so the map is alive and unchanged
        unsafe { self.inner.as_ref().is_key_alive(key) }
    }

    pub fn remove(&mut self, key: &Gc<K>) -> bool {
        // SAFETY: we have unique access to `self`
        unsafe { self.inner.as_mut().remove(key) }
    }
}

impl<K: Trace, V: Trace> Finalize for WeakMap<K, V> {}

// ephemerons are tracked in collector queue
//no extra work needed during trace
unsafe impl<K: Trace + 'static, V: Trace + 'static> Trace for WeakMap<K, V> {
    // SAFETY: trace is a no-op because ephemerons are tracked separately
    unsafe fn trace(&self, _color: TraceColor) {}
    fn run_finalizer(&self) {
        Finalize::finalize(self);
    }
}

impl<K: Trace, V: Trace> Drop for WeakMap<K, V> {
    fn drop(&mut self) {
        // signal the collector that this map is gone so it can drop the inner allocation
        // SAFETY: `inner` pointer remains valid until `is_alive` is set false here
        unsafe { self.inner.as_ref().is_alive.set(false) }
    }
}
