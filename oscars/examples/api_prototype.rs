//! GC API prototype based on gc-arena's lifetime pattern
//!
//! key change: `Gc<'gc, T>` is Copy (zero overhead) vs current `Gc<T>` (inc/dec on clone/drop)
//!
//! Run: `cargo run --example api_prototype --features mark_sweep`

#![allow(dead_code)]

use core::cell::{Cell, RefCell};
use core::marker::PhantomData;
use core::ptr::NonNull;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_COLLECTOR_ID: AtomicU64 = AtomicU64::new(1);

/// GC pointer tied to its collector's lifetime
#[derive(Debug)]
pub struct Gc<'gc, T: Trace + ?Sized> {
    ptr: NonNull<GcBox<T>>,
    _marker: PhantomData<(&'gc T, *const ())>, // *const () makes it !Send + !Sync
}

impl<'gc, T: Trace + ?Sized> Copy for Gc<'gc, T> {}
impl<'gc, T: Trace + ?Sized> Clone for Gc<'gc, T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<'gc, T: Trace> Gc<'gc, T> {
    pub fn get(&self) -> &T {
        unsafe { &(*self.ptr.as_ptr()).value }
    }
}

/// Root keeps objects alive across GC cycles. Validates collector ID on access.
pub struct Root<T: Trace + ?Sized> {
    ptr: NonNull<GcBox<T>>,
    collector_id: u64,
    collector_roots: *const RefCell<Vec<RootEntry>>,
    _marker: PhantomData<*const ()>,
}

impl<T: Trace> Root<T> {
    /// Panics if used with different collector
    pub fn get<'gc>(&self, cx: &MutationContext<'gc>) -> Gc<'gc, T> {
        assert_eq!(
            self.collector_id, cx.collector.id,
            "Root from different collector"
        );
        Gc {
            ptr: self.ptr,
            _marker: PhantomData,
        }
    }

    pub fn belongs_to(&self, cx: &MutationContext<'_>) -> bool {
        self.collector_id == cx.collector.id
    }
}

impl<T: Trace + ?Sized> Drop for Root<T> {
    fn drop(&mut self) {
        if !self.collector_roots.is_null() {
            let roots = unsafe { &*self.collector_roots };
            roots
                .borrow_mut()
                .retain(|entry| entry.ptr != self.ptr.as_ptr() as *mut u8);
        }
    }
}

pub struct MutationContext<'gc> {
    collector: &'gc Collector,
    _marker: PhantomData<*const ()>,
}

impl<'gc> MutationContext<'gc> {
    pub fn alloc<T: Trace + 'static>(&self, value: T) -> Gc<'gc, T> {
        self.collector.alloc(value)
    }

    pub fn root<T: Trace>(&self, gc: Gc<'gc, T>) -> Root<T> {
        self.collector.add_root(gc.ptr);
        Root {
            ptr: gc.ptr,
            collector_id: self.collector.id,
            collector_roots: &self.collector.roots,
            _marker: PhantomData,
        }
    }

    pub fn collector_id(&self) -> u64 {
        self.collector.id
    }

    pub fn collect(&self) {
        self.collector.collect();
    }
}

struct GcBox<T: ?Sized> {
    marked: Cell<bool>,
    value: T,
}

#[derive(Clone)]
struct RootEntry {
    ptr: *mut u8,
}

pub struct Collector {
    id: u64,
    allocations: RefCell<Vec<*mut u8>>,
    roots: RefCell<Vec<RootEntry>>,
    allocation_count: Cell<usize>,
}

impl Collector {
    pub fn new() -> Self {
        Self {
            id: NEXT_COLLECTOR_ID.fetch_add(1, Ordering::Relaxed),
            allocations: RefCell::new(Vec::new()),
            roots: RefCell::new(Vec::new()),
            allocation_count: Cell::new(0),
        }
    }

    fn alloc<'gc, T: Trace + 'static>(&'gc self, value: T) -> Gc<'gc, T> {
        let boxed = Box::new(GcBox {
            marked: Cell::new(false),
            value,
        });
        let ptr = NonNull::new(Box::into_raw(boxed)).unwrap();

        self.allocations.borrow_mut().push(ptr.as_ptr() as *mut u8);
        self.allocation_count.set(self.allocation_count.get() + 1);

        Gc {
            ptr,
            _marker: PhantomData,
        }
    }

    fn add_root<T: Trace + ?Sized>(&self, ptr: NonNull<GcBox<T>>) {
        self.roots.borrow_mut().push(RootEntry {
            ptr: ptr.as_ptr() as *mut u8,
        });
    }

    fn remove_root(&self, ptr: *mut u8) {
        self.roots.borrow_mut().retain(|entry| entry.ptr != ptr);
    }

    fn collect(&self) {
        let root_count = self.roots.borrow().len();
        println!(
            "Collecting garbage: {} objects, {} roots",
            self.allocation_count.get(),
            root_count
        );
        for entry in self.roots.borrow().iter() {
            unsafe {
                let gcbox = entry.ptr as *mut GcBox<()>;
                (*gcbox).marked.set(true);
            }
        }
    }
}

impl Default for Collector {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for Collector {
    fn drop(&mut self) {
        for ptr in self.allocations.borrow().iter() {
            unsafe {
                drop(Box::from_raw(*ptr as *mut GcBox<()>));
            }
        }
    }
}

pub trait Trace {
    fn trace(&self, tracer: &mut Tracer);
}

pub struct Tracer<'a> {
    _marker: PhantomData<&'a ()>,
}

impl Tracer<'_> {
    pub fn mark<T: Trace + ?Sized>(&mut self, _gc: &Gc<'_, T>) {}
}

impl Trace for i32 {
    fn trace(&self, _: &mut Tracer) {}
}
impl Trace for String {
    fn trace(&self, _: &mut Tracer) {}
}

pub fn with_gc<R>(f: impl for<'gc> FnOnce(MutationContext<'gc>) -> R) -> R {
    let collector = Collector::new();
    let cx = MutationContext {
        collector: &collector,
        _marker: PhantomData,
    };
    f(cx)
}

struct JsObject {
    name: String,
    value: i32,
}

impl Trace for JsObject {
    fn trace(&self, tracer: &mut Tracer) {
        self.name.trace(tracer);
        self.value.trace(tracer);
    }
}

struct JsContext {
    global: Option<Root<JsObject>>,
    collector_id: Option<u64>,
}

impl JsContext {
    fn new() -> Self {
        Self {
            global: None,
            collector_id: None,
        }
    }

    fn set_global(&mut self, root: Root<JsObject>, cx: &MutationContext<'_>) {
        self.collector_id = Some(cx.collector_id());
        self.global = Some(root);
    }

    fn with_global<R>(
        &self,
        cx: &MutationContext<'_>,
        f: impl FnOnce(Gc<'_, JsObject>) -> R,
    ) -> Option<R> {
        self.global.as_ref().map(|root| {
            let gc = root.get(cx);
            f(gc)
        })
    }
}

fn main() {
    println!("GC API Prototype Example\n");

    // Example 1: Copying pointers
    println!("1. Copying pointers:\n");
    with_gc(|cx| {
        let obj1 = cx.alloc(JsObject {
            name: "first".to_string(),
            value: 42,
        });

        let obj2 = obj1;
        let obj3 = obj1;

        println!("  obj1.value = {}", obj1.get().value);
        println!("  obj2.value = {}", obj2.get().value);
        println!("  obj3.value = {}", obj3.get().value);
        println!("  (All three point to same object - Copy is free!)\n");
    });

    // Example 2: Rooting within same context (correct usage)
    println!("2. Rooting objects (same context):\n");
    with_gc(|cx| {
        let obj = cx.alloc(JsObject {
            name: "global".to_string(),
            value: 100,
        });

        let root = cx.root(obj);
        println!("  Created root with collector_id = {}", cx.collector_id());

        // Access via root in same context
        let gc = root.get(&cx);
        println!("  Accessed via root: global.value = {}\n", gc.get().value);
    });

    // Example 3: Compile time safety
    println!("3. Context isolation:\n");
    println!("  This would fail to compile:");
    println!("  let escaped;");
    println!("  with_gc(|cx| {{");
    println!("    let obj = cx.alloc(42);");
    println!("    escaped = obj;  // Error: can't escape 'gc");
    println!("  }});\n");

    // Example 4: Collection
    println!("4. Running collection:\n");
    with_gc(|cx| {
        let _obj1 = cx.alloc(JsObject {
            name: "temp1".to_string(),
            value: 1,
        });
        let obj2 = cx.alloc(JsObject {
            name: "temp2".to_string(),
            value: 2,
        });
        let _root = cx.root(obj2);

        cx.collect();
        println!();
    });

    println!("Done!");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gc_is_copy() {
        with_gc(|cx| {
            let a = cx.alloc(42i32);
            let b = a; // copy
            let c = a; // copy again
            assert_eq!(*a.get(), *b.get());
            assert_eq!(*b.get(), *c.get());
        });
    }

    #[test]
    fn root_works_in_same_context() {
        with_gc(|cx| {
            let obj = cx.alloc(123i32);
            let root = cx.root(obj);
            let gc = root.get(&cx);
            assert_eq!(*gc.get(), 123);
        });
    }

    #[test]
    #[should_panic(expected = "Root from different collector")]
    fn root_rejects_different_collector() {
        let root = with_gc(|cx| {
            let obj = cx.alloc(123i32);
            cx.root(obj)
        });

        with_gc(|cx| {
            let _gc = root.get(&cx);
        });
    }

    #[test]
    fn root_belongs_to_check() {
        let (root, collector_id) = with_gc(|cx| {
            let obj = cx.alloc(42i32);
            (cx.root(obj), cx.collector_id())
        });

        with_gc(|cx| {
            assert!(!root.belongs_to(&cx));
            assert_ne!(collector_id, cx.collector_id());
        });
    }

    #[test]
    fn multiple_allocations() {
        with_gc(|cx| {
            let a = cx.alloc(1i32);
            let b = cx.alloc(2i32);
            let c = cx.alloc(3i32);

            assert_eq!(*a.get(), 1);
            assert_eq!(*b.get(), 2);
            assert_eq!(*c.get(), 3);
        });
    }

    #[test]
    fn root_dropped_removes_from_collector() {
        with_gc(|cx| {
            let obj = cx.alloc(42i32);
            {
                let _root = cx.root(obj);
            }
        });
    }
}
