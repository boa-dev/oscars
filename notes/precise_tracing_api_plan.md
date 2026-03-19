# Precise-Tracing API Plan (Post `gc_allocator`)

Date: 2026-03-19

## Context

This note proposes a concrete API-design and implementation plan for the
GC redesign track after removing the `Collector: Allocator` experiment
(`#54`).

It builds on:

- `notes/gc_api_models.md` (model-family investigation for boa#2631)
- `docs/boa_gc_api_surface.md` (current Boa-facing GC contract)
- Oscars integration tracker issues (`#26`, `#27`, `#28`, `#30`)

The objective is to design a GC API that does **not** rely on root/reference
counting for liveness, while preserving precise tracing semantics and providing
an incremental integration path into Boa.

## Problem statement

Today, root/reference counts are coupled with public pointer ergonomics and
collector internals. This coupling increases implementation complexity and
makes it harder to evolve collector strategy.

For a redesign to be integration-ready, we need:

1. A pointer/rooting API where liveness is derived from precise tracing.
2. Explicit invariants for weak/ephemeron/finalizer behavior.
3. A staged migration path that can run behind feature gates in Boa.

## Design goals

1. Precise tracing as the source of truth for liveness.
2. Context-safe pointer model (no accidental cross-context sharing).
3. Weak and ephemeron behavior parity with current Boa semantics.
4. Finalization semantics that are explicit and testable.
5. Incremental adoption (no big-bang replacement).

## Non-goals (for this phase)

1. Generational or incremental collector algorithm rollout.
2. Concurrent collector implementation.
3. Full allocator-framework redesign in the same milestone.
4. Public engine-wide API churn in one PR.

## Core invariants

### I1. Tracing invariant

Any object reachable from roots through strong edges must be marked alive in the
same collection cycle.

### I2. Rooting invariant

Rooting API must make temporary and long-lived roots explicit, and prevent
silent loss of roots due to API misuse.

### I3. Weak invariant

Weak handles do not keep referents alive. Upgrades succeed only if referent is
alive at upgrade time.

### I4. Ephemeron invariant

Ephemeron value is considered reachable only when key is independently
reachable by strong tracing rules.

### I5. Finalizer invariant

Finalization happens before drop/reclaim. If finalization can make objects
reachable again, collection ordering must prevent use-after-free.

### I6. Teardown invariant

Collector-drop paths must preserve safety ordering and avoid freeing objects
that may still be referenced by finalizer-triggered graph activity.

## Proposed API direction (high-level)

1. Keep user-facing `Gc<T>` ergonomics close to current behavior where possible.
2. Shift internal liveness authority to tracing rather than root/refcount
   arithmetic.
3. Make rooting scopes/handles explicit in API boundaries where ambiguity
   exists.
4. Keep weak and ephemeron APIs stable at surface level while tightening
   internal reachability contracts.

This allows incremental migration while reducing dependence on reference-count
based root detection.

## Implementation phases

### Phase 1: API contract and invariant harness

1. Add a dedicated invariant checklist and edge-case test matrix.
2. Expand coverage for rooting misuse, weak upgrades, ephemeron pruning, and
   finalizer/resurrection-sensitive flows.
3. Define acceptance criteria for parity against `docs/boa_gc_api_surface.md`.

Deliverable:

- Invariant-driven test suite and API-diff checklist.

### Phase 2: Oscars prototype changes

1. Prototype precise-tracing-first liveness flow in Oscars internals.
2. Keep changes in reviewable slices (pointer semantics, tracing hooks, weak/
   ephemeron behavior).
3. Ensure Miri-clean behavior throughout.

Deliverable:

- Prototype branch with passing invariants and stress tests.

### Phase 3: Boa integration path (feature-gated)

1. Map prototype API to Boa `core/gc` surface.
2. Integrate behind unstable gate with fallback to existing path.
3. Validate on Boa-relevant workloads, not only microbenchmarks.

Deliverable:

- Gated integration slices plus benchmark and migration notes.

## Validation strategy

1. Correctness
   - Targeted unit/regression tests for each invariant.
   - Workspace test suites pass.
2. Safety
   - Miri coverage for critical paths.
3. Performance
   - Boa workload benchmarks plus focused GC stress tests.
4. Integration confidence
   - API parity matrix against `docs/boa_gc_api_surface.md`.

## Risk management

1. Scope creep
   - Keep non-goals explicit and enforce slice-based PRs.
2. Unsoundness regressions
   - Require invariant tests before merging behavioral changes.
3. Integration disruption
   - Use feature-gated rollout and preserve fallback path.

## Relationship to active tracker work

This plan is intended to feed directly into:

- `#26` Tracking issue for Boa integration
- `#27` Coverage of `boa_gc` API surface area
- `#28` Integration into Boa
- `#30` Benchmark MarkSweepGarbageCollector in Boa with `arena3` allocator

And it is explicitly aligned with the `#54` direction to remove the
`gc_allocator` supertrait experiment.
