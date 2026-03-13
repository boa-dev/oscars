// Both collectors use the exact same `Trace` types
// NOTE: `empty_trace!` and `custom_trace!` hardcode `mark_sweep` paths
// This works now but will silently break if the types ever diverge.
pub use crate::collectors::mark_sweep::trace::{Finalize, Trace, TraceColor};
