# leet_jobs2 — Execution Notes

This file is for implementation notes that should not live in Rust source
comments.

Rust source comments should explain the Rust design directly: ownership,
synchronization, ordering, panic policy, and public API contracts. They should
not mention legacy file names, old call paths, or translation history.

Use this file when implementation work needs to track lineage, audit decisions,
or compare behavior against the legacy job system.

---

## Comment Policy

Good source comments should answer one of these questions:

- What invariant is protected here?
- What race is this lock, atomic ordering, or recheck preventing?
- Why does this operation have to happen before another operation?
- Why is a panic preferable to silently accepting misuse?
- Why is this public API intentionally absent or unavailable in this pass?

Avoid comments that merely narrate the code. A comment like "increment the
counter" is usually noise. A comment explaining why the counter must be
incremented before queueing is valuable.

---

## Legacy Relationship Notes

When tracking behavior against the legacy implementation, keep notes here or in
`leet_jobs2_internals.md`.

Use this file for:

- Rust-to-legacy behavior mapping
- audit notes during a pass
- reasons a Rust design intentionally differs from the old implementation
- references to old files or old function names
- migration risks that should not appear in production source comments

Do not turn this file into an implementation dumping ground. If a note changes
the design contract, update `leet_jobs2_internals.md` or
`leet_jobs2_execution_passes.md` as well.

---

## Current Audit Notes

- Rust source comments should describe `CounterEntry` lifetime in terms of
  `Arc<CounterEntry>` ownership, not in terms of legacy refcount machinery.
- Waiting-list comments should describe the locked recheck race directly, not by
  pointing at an old file.
- Worker and dispatcher comments should describe the public v1 shutdown policy:
  explicit, idempotent, wakes workers, joins workers, and does not drain queued
  work.
- Builder comments should describe fence misuse and continuation linking as Rust
  API invariants.
- Parallel-for comments should describe team jobs, batch claiming, and epilogue
  ownership without relying on translation history.
