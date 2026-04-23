# Part 3: Object Layout and Header Design

Note author: shruti2522

## Background

Every GC managed object needs metadata so the collector can do its job. This note looks at how Wasmtime structures object headers and what we can take from that for boa_gc.

## What Wasmtime Does

Every GC object in Wasmtime starts with a `VMGcHeader`:

```rust
#[repr(C)]
pub struct VMGcHeader {
    type_index: u32,
    gc_metadata: GcMetadata,
}
```

A few things stand out here. The header is always at the start of every allocation. The type index is a plain `u32` used for runtime type checks and downcasts. The metadata slot is used differently depending on the collector: DRC stores a reference count there, the null collector leaves it unused and a future tracing collector would use it for mark bits or a forwarding pointer. Types are interned at the engine level, so only the index lives per object, keeping the header small.

After the header comes the payload. Structs store fields inline. Arrays store a `u32` length at a fixed offset, then elements. The length being at a fixed offset matters because it allows bounds checks without a layout lookup on every array access.

## Type IDs

Wasmtime uses `u32` for type IDs and didn't try to use a smaller type to save space. For JavaScript this is the right call. Shapes are created dynamically and the count can grow large in real programs. `u32` is a reasonable starting point until we have data showing otherwise.

## What this means for boa_gc

### The Header

Every heap allocated object needs a fixed header. The header must be `#[repr(C)]` for predictable layout, the same structure for all objects and small but with space reserved for future collectors. Each allocation is laid out as a header followed immediately by the value:

```rust
#[repr(C)]
pub struct GcBox<T: Trace> {
    header: GcHeader,
    value: T,
}
```

### Reserve Space Now, Use Later

The most important lesson here is to reserve header space even if the first collector does not use all of it. Adding a header field later means touching every allocation site, every size calculation and every unsafe offset in the codebase. An extra 8 bytes per object is negligible compared to the object payload. Moving collectors need forwarding pointers, generational GC needs age bits. Reserve the bits now:

```rust
pub struct GcHeader {
    shape_id: u32,
    gc_flags: u32,  // MARKED = 1 << 0, FORWARDED = 1 << 1, age bits, etc.
}
```

Better to have unused space than to redesign the allocator later.

### Shape Registry

JS objects have dynamic shapes. We need a registry to keep per object headers small:

```rust
pub struct ShapeRegistry {
    shapes: Vec<Shape>,
    shape_map: HashMap<PropertyLayout, ShapeId>,
}

#[derive(Copy, Clone)]
pub struct ShapeId(u32);
```

The object header stores only the `u32` shape ID. The full layout descriptor lives in the registry on `GcContext`.

### Arrays

Following Wasmtime's pattern of a fixed offset for length:

```rust
#[repr(C)]
pub struct JsArray {
    header: GcHeader,
    length: u32,
    capacity: u32,
    elements: *mut JsValue,
}
```

## Conclusion

The header design is foundational. Getting it right early avoids expensive refactoring later. Define `GcHeader` now with reserved space, use `u32` for shape IDs, keep the layout fixed and `#[repr(C)]` and plan for future collectors by reserving bits for forwarding pointers and age tracking even if unused today.
