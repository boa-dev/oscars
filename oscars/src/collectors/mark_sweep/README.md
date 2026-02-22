# Mark sweep collector

This is a basic mark-sweep collector using an underlying arena allocator.

## TODO list

- [x] Support weak maps
- [x] Add Tests


## Areas of improvement

The overhead on a single allocation honestly feels a bit high. This may be worthwhile
for now for performance gains and general API, but we should really measure and determine
just how much overhead is being added.

Currently, there is a line drawn between the allocator and the GcBox. This creates very,
very awkward naming (ArenaPointer, ArenaHeapItem, GcBox, etc.). We may be able to combine
the general functionality of the ArenaHeapItem, and GcBox. But also, that would then
restrict the potential ability to switch out allocators as easily ... to be determined.

