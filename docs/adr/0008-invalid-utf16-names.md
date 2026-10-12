# ADR 0008: Invalid UTF-16 names are stored lossy, with a per-node flag

- Status: Accepted · 2026-10-13

## Context

NTFS and Win32 file names are sequences of UTF-16 code units with no validity guarantee: lone surrogates occur in real file systems and in hostile images. The tree stores names as `Box<str>` (ADR 0004). A name that cannot be decoded must neither panic nor abort a scan (`docs/never-crash.md`), and the user should still see that the entry exists.

## Decision

- Names are decoded with `model::name_from_wide(&[u16]) -> (String, bool)`: every unpaired surrogate becomes U+FFFD and the boolean reports whether any replacement happened.
- `NodeData::name_lossy` records that flag per node. `Tree::add_node_from_wide` decodes and sets it (OR-ed with a flag already present in the supplied data).
- The original code units are not kept. Operations that need the exact on-disk name (for example deleting such an entry) must not rely on the stored name; they are refused for lossy nodes until a separate decision adds a raw-name side table.
- The UI may mark lossy names; the flag is serializable through the existing node data.

## Consequences

- No new node field cost beyond one `bool` in `NodeData`; lossy names are rare, so no side storage is needed.
- Lossy names from different entries may collide in display; identity is the `NodeId`, never the name.
- The NTFS parser and the portable scanner (for non-Unicode OS names) should use this path.
