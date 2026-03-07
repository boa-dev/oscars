//! A garbage collected cell implementation

pub use crate::collectors::mark_sweep::cell::{
    BorrowError, BorrowMutError, GcRef, GcRefCell, GcRefMut,
};
