# Architecture Decision Records

Decisions are recorded briefly: context → decision → consequences. Do not revisit an accepted decision without a new ADR that references the old one (`Supersedes`). Numbering is sequential, files are `NNNN-kebab-title.md`, status is `Accepted` / `Superseded` / `Proposed`.

| No. | Decision | Status |
|---|---|---|
| [0001](0001-rust-only-native-egui.md) | Pure Rust, native egui/eframe on wgpu, no webview | Accepted |
| [0002](0002-mft-unsafe-isolation.md) | MFT fast path and isolating `unsafe` in `dirustat-ntfs` | Accepted |
| [0003](0003-everything-is-a-command.md) | Everything is a command; clients are equal | Accepted |
| [0004](0004-arena-tree-model.md) | Arena tree with `u32` ids and iterative aggregation | Accepted |
| [0005](0005-layers-and-agent-workflow.md) | Layers, xtask enforcement, agent development workflow | Accepted |
| [0006](0006-naming-and-licensing.md) | Name diRUSTat/dirustat, licence MIT OR Apache-2.0, clean-room | Accepted |
| [0007](0007-english-only-repository.md) | All repository text is English; chat uses the user's language | Accepted |
| [0008](0008-invalid-utf16-names.md) | Invalid UTF-16 names: lossy `String` plus a `name_lossy` flag | Accepted |
