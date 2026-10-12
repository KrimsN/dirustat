# ADR 0001: Pure Rust, native egui/eframe on wgpu

- Status: Accepted · 2026-10-13

## Context

We need a fast native disk analyzer with a large interactive treemap (hundreds of thousands of rectangles) and full agent control. The sibling storytold projects (photocraft, wordcraft) chose native egui/eframe on wgpu without Tauri/Electron.

## Decision

- All code is Rust. No hand-written JS/TS. No Tauri, wry, Electron or webview.
- The UI is egui/eframe on wgpu (`dirustat-ui`, layer L4). The treemap is drawn by our own mesh/cache on top of the egui painter.
- The UI is thin: state lives in serde structures, actions are commands (ADR 0003).
- A web build is not a goal; `model` and `treemap` stay wasm-compatible (demo, reuse).

## Consequences

- One stack for developers and agents, simple releases (a single binary).
- Non-standard widgets (a tree with columns, the treemap) are written by us; row virtualization is mandatory.
- Accessibility is bounded by what egui/AccessKit offer; we follow their updates.
- Alternatives (Tauri, Slint, iced) were rejected: a webview contradicts the goal; Slint/iced have less experience in sibling projects and less mature widgets. A new ADR can revisit this if egui hits a treemap performance wall.
