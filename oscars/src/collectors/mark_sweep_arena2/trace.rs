// Both collectors use the exact same trace API types today.
// Helper macros resolve through crate-level `gc_trace`, so collector modules
// do not hardcode paths to a specific implementation.
pub use crate::collectors::mark_sweep::trace::{Finalize, Trace, TraceColor};
