// Both collectors use the exact same `Trace` types.
// NOTE: `empty_trace!` and `custom_trace!` resolve through mark_sweep paths.
// This works today because mark_sweep2 depends on mark_sweep.
pub use crate::collectors::mark_sweep::trace::{Finalize, Trace, TraceColor};
