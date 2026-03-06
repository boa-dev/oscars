mod ephemeron;
mod gc_box;
mod gc_header;
mod vtable;

pub(crate) use ephemeron::Ephemeron;
pub(crate) use vtable::{DropFn, TraceFn, VTable, vtable_of};

pub use self::gc_box::{GcBox, NonTraceable, WeakGcBox};
