# Architecture Decision Records

Решения фиксируются коротко: контекст → решение → последствия. Принятое не пересматривай без нового ADR, который ссылается на старый (`Supersedes`). Нумерация сквозная, файл `NNNN-kebab-title.md`, статус: `Accepted` / `Superseded` / `Proposed`.

| № | Решение | Статус |
|---|---|---|
| [0001](0001-rust-only-native-egui.md) | Чистый Rust, нативный egui/eframe на wgpu, без webview | Accepted |
| [0002](0002-mft-unsafe-isolation.md) | Быстрый MFT-путь и изоляция `unsafe` в `dirustat-ntfs` | Accepted |
| [0003](0003-everything-is-a-command.md) | Всё — команда; клиенты равноправны | Accepted |
| [0004](0004-arena-tree-model.md) | Arena-дерево с `u32` id и итеративной агрегацией | Accepted |
| [0005](0005-layers-and-agent-workflow.md) | Слои, проверка в xtask, процесс агентной разработки | Accepted |
| [0006](0006-naming-and-licensing.md) | Имя diRUSTat/dirustat, лицензия MIT OR Apache-2.0, clean-room | Accepted |
