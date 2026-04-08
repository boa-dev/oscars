# Part 4: Precise Root Tracking

Note author: shruti2522

## Background

The collector needs to find all live objects. This means identifying "roots", objects directly reachable without following pointers. Wasmtime handles this with precise stack maps generated at compile time. We do not have a JIT, so we need another approach.

## Why Precise Roots Matter

Conservative stack scanning blocks moving collectors. If any integer on the stack looks like a heap address, the collector cannot safely move that object. This rules out compacting, generational and copying collectors. It also causes false retentions, where an integer that happens to look like a pointer keeps an object alive when nothing actually references it.

Precise roots avoid both problems.

## The Problem for oscars

Wasmtime uses Cranelift to generate stack maps at compile time. A stack map records which stack slots hold live GC references at each potential collection point. We cannot do this without a JIT.

Two practical approaches exist.

### Approach 1: Shadow Stack

Maintain a separate stack of GC pointers alongside the native call stack. Push on allocation, pop on scope exit.

Pros: precise, simple to implement, no compiler needed.

Cons: manual push/pop at every allocation point, easy to forget, leads to bugs.

### Approach 2: Handle Table (Recommended)

Store all GC references in a table on the Context. Stack frames hold indices into this table, not raw pointers.

```rust
pub struct HandleTable {
    entries: Vec<TableEntry>,
    free_list: Vec<u32>,
}

struct TableEntry {
    ptr: *mut GcHeader,
    refcount: u32,
}

#[derive(Copy, Clone)]
pub struct Handle<T> {
    index: u32,
    _marker: PhantomData<T>,
}
```

The collector only needs to scan the table:

```rust
fn collect(&mut self, ctx: &mut Context) {
    let roots: Vec<*mut GcHeader> = ctx.handle_table.iter_live().collect();
    self.mark_sweep(&roots);
}
```

When the collector moves an object, it updates the table entry. All existing handles continue to work with no other changes needed:

```rust
fn compact(&mut self, handle_table: &mut HandleTable) {
    for entry in &mut handle_table.entries {
        if entry.refcount > 0 {
            let new_ptr = self.copy_object(entry.ptr);
            entry.ptr = new_ptr;
        }
    }
}
```

Pros: precise roots without manual tracking, safe to move objects, automatic cleanup when handles drop, no compiler needed.

Cons: indirection overhead on every access, memory overhead for table storage

## Comparison

| Feature | Shadow Stack | Handle Table | Stack Maps (JIT) |
|---|---|---|---|
| Precision | Yes | Yes | Yes |
| Manual tracking | Required | Automatic | Automatic |
| Moving GC support | Yes | Yes | Yes |
| Implementation complexity | Low | Medium | High |

## Conclusion

Use the handle table for the prototype. It gives precise roots without JIT support and keeps the door open for a compacting or generational collector later. Conservative scanning should not be used at all, it blocks future improvements. When a JIT is added later, stack maps can replace the handle table for JIT code while the interpreter keeps using the table.

Precise root tracking is a hard requirement for compacting and generational GC. Getting this right early matters a lot.
