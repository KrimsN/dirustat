# diRUSTat

Быстрый анализатор дискового пространства на чистом Rust: «куда делось место?» — в стиле WinDirStat, со скоростью WizTree.

> Статус: **M0 (bootstrap)** — каркас workspace, правила и документация для агентной разработки. Приложение пока печатает баннер; первое окно — веха M1. См. [ROADMAP.md](ROADMAP.md).

## Цели

- **Паритет по функциям с WinDirStat:** дерево каталогов, treemap с cushion-шейдингом, список типов файлов с цветами, очистки (cleanups), поиск, фильтры.
- **Скорость уровня WizTree:** прямое чтение NTFS MFT (быстрый путь, Windows), параллельный переносимый обход для остальных ФС.
- **Нативно и без оболочек:** egui/eframe на wgpu. Без Tauri/Electron/webview, без JS/TS.
- **Каждая функция доступна агентам:** все действия — команды; JSON control-канал, CLI и MCP-сервер.
- **Никогда не падает:** без `unwrap`/`panic`/`unsafe` (кроме изолированного MFT-крейта), враждебные входы дают ошибки.
- **Безопасность данных:** сканирование только читает; удаление — через корзину и подтверждение.

## Быстрый старт

```sh
cargo run -p dirustat                 # десктопное приложение (пока баннер)
cargo run -p dirustat-cli             # headless CLI
cargo xtask ci                        # полный гейт качества
```

Требуется Rust stable (см. `rust-toolchain.toml`). Подробности: [docs/development.md](docs/development.md).

## Для агентов и контрибьюторов

Начните с [AGENTS.md](AGENTS.md) (для Claude Code — `CLAUDE.md`, он подключает `AGENTS.md`), затем `plan/STATUS.md`. Архитектура — [docs/architecture.md](docs/architecture.md), правила — [docs/contributing.md](docs/contributing.md).

## Лицензия

MIT OR Apache-2.0 ([LICENSE-MIT](LICENSE-MIT), [LICENSE-APACHE](LICENSE-APACHE)). WinDirStat и WizTree — торговые марки их владельцев; проект с ними не связан и реализован clean-room ([docs/clean-room.md](docs/clean-room.md)).
