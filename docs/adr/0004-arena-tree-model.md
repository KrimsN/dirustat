# ADR 0004: Arena tree with `u32` ids

- Status: Accepted · 2026-10-13

## Context

The file tree has millions of nodes, is built by the scanner and read by the UI and treemap. Per-node pointers/`Rc`/`Box` are expensive in memory and speed, and recursion is dangerous on deep trees.

## Decision

- A flat `Vec<Node>`; `NodeId(u32)`; links `parent`, `first_child`, `last_child`, `next_sibling`. A parent is added before its children.
- Size aggregation is one reverse pass, without recursion, using saturating addition.
- The limit is `MAX_NODES` (about 4.29 billion); beyond it `ModelError::TooManyNodes`.
- Names are `Box<str>`; paths are built on demand, not stored.
- After a scan the tree is immutable and published as an `Arc` with a revision; caches are keyed by revision.

## Consequences

- Compact and fast, convenient for parallel construction (local subtrees merge into the arena).
- Nodes cannot be removed in place: after destructive actions we rescan the subtree and publish a new revision (M4).
- Node fields (allocated size, counters, mtime) can be extended without changing the id scheme.
