use hashbrown::HashMap;

use crate::{
    alloc::arena2::{ArenaPointer, ErasedHeapItem},
    collectors::mark_sweep::{
        Finalize, MarkSweepGarbageCollector, TraceColor,
        internals::Ephemeron,
        trace::Trace,
    },
};

use super::Gc;

// must be registered via `register_weak_map` or reads after GC will panic
pub struct WeakMap<K: Trace + Clone + 'static, V: Trace + 'static> {
    entries: HashMap<usize, ArenaPointer<'static, Ephemeron<K, V>>>,

    #[cfg(debug_assertions)]
    registered: bool,
}

impl<K: Trace + Clone, V: Trace> WeakMap<K, V> {
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
            #[cfg(debug_assertions)]
            registered: false,
        }
    }

    // set by register_weak_map so the assert in get/is_key_alive can fire
    #[cfg(debug_assertions)]
    pub(crate) fn mark_registered(&mut self) {
        self.registered = true;
    }

    #[cfg(not(debug_assertions))]
    pub(crate) fn mark_registered(&mut self) {}

    pub fn insert(
        &mut self,
        key: &Gc<K>,
        value: V,
        collector: &mut MarkSweepGarbageCollector,
    ) {
        let key_addr = key.inner_ptr.as_non_null().as_ptr() as usize;

        if let Some(ephemeron_ptr) = self.entries.get_mut(&key_addr) {
            unsafe { ephemeron_ptr.as_inner_mut() }.set_value(value, &collector.state);
            return;
        }

        let ephemeron = Ephemeron::new_in((**key).clone(), value, collector);
        let ephemeron_ptr = collector.alloc_epemeron_with_collection(ephemeron);

        self.entries.insert(key_addr, ephemeron_ptr);
    }

    pub fn get(&self, key: &Gc<K>) -> Option<&V> {
        // without registration dead entries point into freed memory
        #[cfg(debug_assertions)]
        debug_assert!(
            self.registered || self.entries.is_empty(),
            "WeakMap must be registered with the collector before use"
        );
        let key_addr = key.inner_ptr.as_non_null().as_ptr() as usize;
        self.entries.get(&key_addr)
            .map(|ephemeron_ptr| ephemeron_ptr.as_inner_ref().value())
    }

    pub fn is_key_alive(&self, key: &Gc<K>) -> bool {
        #[cfg(debug_assertions)]
        debug_assert!(
            self.registered || self.entries.is_empty(),
            "WeakMap must be registered with the collector before use"
        );
        let key_addr = key.inner_ptr.as_non_null().as_ptr() as usize;
        self.entries.contains_key(&key_addr)
    }

    pub fn remove(&mut self, key: &Gc<K>) -> Option<V>
    where
        V: Clone,
    {
        let key_addr = key.inner_ptr.as_non_null().as_ptr() as usize;
        self.entries.remove(&key_addr)
            .map(|ephemeron_ptr| ephemeron_ptr.as_inner_ref().value().clone())
    }

    // clean up dead entries after collector sweep
    pub(crate) fn prune_dead_entries(&mut self) {
        self.entries.retain(|_, ephemeron_ptr| {
            // SAFETY: Memory is valid until next collector step
            // We only read the dropped flag
            let heap_item = unsafe {
                ephemeron_ptr.as_ptr().cast::<ErasedHeapItem>().as_ref()
            };
            !heap_item.is_dropped()
        });
    }
}

impl<K: Trace + Clone, V: Trace> Default for WeakMap<K, V> {
    fn default() -> Self {
        Self::new()
    }
}

impl<K: Trace + Clone, V: Trace> Finalize for WeakMap<K, V> {}

// ephemerons are tracked in collector queue
//no extra work needed during trace
unsafe impl<K: Trace + Clone + 'static, V: Trace + 'static> Trace for WeakMap<K, V> {
    unsafe fn trace(&self, _color: TraceColor) {}
    fn run_finalizer(&self) {
        Finalize::finalize(self);
    }
}
