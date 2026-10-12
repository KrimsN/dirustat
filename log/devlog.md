# Dev log

Новые записи — сверху. Формат — `docs/progress-docs.md`.

## 2026-10-13 · M0 · Claude
- Что вошло: инициализирован репозиторий diRUSTat. Workspace (7 крейтов + 2 приложения + xtask), lints «never crash» (deny unwrap/expect/panic/индексации, `unsafe` запрещён кроме `dirustat-ntfs`), `cargo xtask layers|ci`, CI в GitHub Actions, `deny.toml`, бюджеты `perf/budgets.toml`. Документация для агентов: `AGENTS.md`, `CLAUDE.md`, `docs/*`, ADR 0001–0006, `plan/*`. Конфигурация Claude Code: `.claude/commands`, `.claude/agents`.
- Основа подхода: изучены репозитории storytold (photocraft, wordcraft, craft-libs): AGENTS.md/CLAUDE.md, слои с xtask, «всё — команда», control-канал/MCP, never-crash, devlog и живые документы прогресса. Приватный репозиторий `craftrules` недоступен — его стандарты воссозданы по ссылкам в публичных файлах.
- Числа: тесты — model 3, treemap 2, scan/engine/control/ui/ntfs по 1–2, xtask 1; паритет 0/34 и 0/12.
- Проверено: `cargo xtask ci` — зелёный (wasm-core пропущен локально: target не установлен).
- Still open: M1.1 (модель), зависимости M1, решение человека о резервировании имени `dirustat`.
