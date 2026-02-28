use hashbrown::HashMap;

use crate::{
    alloc::arena2::{ArenaPointer, ErasedHeapItem},
    collectors::mark_sweep::{
        Finalize, MarkSweepGarbageCollector, TraceColor, internals::Ephemeron, trace::Trace,
    },
};

use super::Gc;

// type erased trait so the collector can prune any WeakMap without knowing K/V
pub(crate) trait ErasedWeakMap {
    fn prune_dead_entries(&mut self);
}

// the actual weak map store, managed by the collector
//
// TODO: a HashTable might be a better approach here
struct WeakMapInner<K: Trace + 'static, V: Trace + 'static> {
    entries: HashMap<usize, ArenaPointer<'static, Ephemeron<K, V>>>,
}

impl<K: Trace, V: Trace> WeakMapInner<K, V> {
    fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    fn insert(&mut self, key: &Gc<K>, value: V, collector: &mut MarkSweepGarbageCollector) {
        let key_addr = key.inner_ptr.as_non_null().as_ptr() as usize;

        // Drop the old entry before allocating a new one to prevent the old
        // ephemeron from leaking into the collector queue when a value is updated
        self.entries.remove(&key_addr);

        let ephemeron = Ephemeron::new_in(key, value, collector);
        let ephemeron_ptr = collector.alloc_epemeron_with_collection(ephemeron);

        // TODO: maybe insert should take an ephemeron instead of key/value pair
        self.entries.insert(key_addr, ephemeron_ptr);
    }

    fn get(&self, key: &Gc<K>) -> Option<&V> {
        let key_addr = key.inner_ptr.as_non_null().as_ptr() as usize;
        self.entries
            .get(&key_addr)
            .map(|p| p.as_inner_ref().value())
    }

    fn is_key_alive(&self, key: &Gc<K>) -> bool {
        let key_addr = key.inner_ptr.as_non_null().as_ptr() as usize;
        self.entries.contains_key(&key_addr)
    }

    fn remove(&mut self, key: &Gc<K>) -> Option<V>
    where
        V: Clone,
    {
        let key_addr = key.inner_ptr.as_non_null().as_ptr() as usize;
        // the backing ephemeron stays in the collector queue and gets swept
        // when the key is collected
        self.entries
            .remove(&key_addr)
            .map(|p| p.as_inner_ref().value().clone())
    }
}

impl<K: Trace, V: Trace> ErasedWeakMap for WeakMapInner<K, V> {
    fn prune_dead_entries(&mut self) {
        self.entries.retain(|_, ephemeron_ptr| {
            // SAFETY: Memory is valid until next collector step
            // We only read the dropped flag
            let heap_item = unsafe { ephemeron_ptr.as_ptr().cast::<ErasedHeapItem>().as_ref() };
            !heap_item.is_dropped()
        });
    }
}

// simple map that prunes entries automatically when their keys are collected
//
// the collector owns the actual data, this is just a thin pointer
// wrapper that stays valid as long as the collector does
pub struct WeakMap<K: Trace + 'static, V: Trace + 'static> {
    // raw pointer to collector owned memory
    inner: *mut WeakMapInner<K, V>,
}

impl<K: Trace, V: Trace> WeakMap<K, V> {
    // create a new map and give the collector ownership of its memory
    pub fn new(collector: &mut MarkSweepGarbageCollector) -> Self {
        let boxed: rust_alloc::boxed::Box<WeakMapInner<K, V>> =
            rust_alloc::boxed::Box::new(WeakMapInner::new());
        // get a raw pointer that stays valid even after the box is moved
        let inner: *mut WeakMapInner<K, V> = rust_alloc::boxed::Box::into_raw(boxed);
        // SAFETY: we just got this from into_raw, and we are giving ownership to the collector
        let erased: rust_alloc::boxed::Box<dyn ErasedWeakMap> =
            unsafe { rust_alloc::boxed::Box::from_raw(inner) };
        collector.weak_maps.push(erased);
        Self { inner }
    }

    pub fn insert(&mut self, key: &Gc<K>, value: V, collector: &mut MarkSweepGarbageCollector) {
        // SAFETY: we have a mut reference to the collector, so the memory is alive
        unsafe { (*self.inner).insert(key, value, collector) }
    }

    pub fn get(&self, key: &Gc<K>) -> Option<&V> {
        // SAFETY: same as insert
        unsafe { (*self.inner).get(key) }
    }

    pub fn is_key_alive(&self, key: &Gc<K>) -> bool {
        // SAFETY: same as insert
        unsafe { (*self.inner).is_key_alive(key) }
    }

    pub fn remove(&mut self, key: &Gc<K>) -> Option<V>
    where
        V: Clone,
    {
        // SAFETY: same as insert
        unsafe { (*self.inner).remove(key) }
    }
}

impl<K: Trace, V: Trace> Finalize for WeakMap<K, V> {}

// ephemerons are tracked in collector queue
//no extra work needed during trace
unsafe impl<K: Trace + 'static, V: Trace + 'static> Trace for WeakMap<K, V> {
    unsafe fn trace(&self, _color: TraceColor) {}
    fn run_finalizer(&self) {
        Finalize::finalize(self);
    }
}
