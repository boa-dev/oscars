# Conclusion

date: 2026-03-17
author: shruti2522

We have decided not to go ahead with `Collector: Allocator`. One of the main
reasons is that it requires the collector to be non-moving, but the whole point
of this redesign is to move Boa towards a moving GC, so implementing a feature
that fundamentally depends on objects never moving works against that goal. The
rest of the reasoning is documented in `collector_allocator_supertrait.md`.
