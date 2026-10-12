# Разработка: сборка, тесты, запуск, отладка

> Статус: 2026-10-13 · ревизия 1.

## Инструменты

- Rust stable (закреплён `rust-toolchain.toml`: clippy, rustfmt). MSRV — `rust-version` в корневом `Cargo.toml`.
- Windows: MSVC build tools. Для wasm-проверки L0: `rustup target add wasm32-unknown-unknown`.
- Опционально: `cargo-nextest` (быстрые тесты), `cargo-deny` (лицензии), `cargo-fuzz` (fuzz парсеров).

## Основные команды

```sh
cargo build --workspace
cargo run -p dirustat                         # приложение
cargo run -p dirustat-cli -- <подкоманда>     # CLI
cargo test -p <крейт> --lib --tests           # быстрый цикл (без doctests)
cargo nextest run                             # параллельно по бинарникам (если установлен)
cargo xtask layers                            # слои зависимостей
cargo xtask ci                                # fmt + clippy -D warnings + test + wasm-core + layers
cargo deny check                              # лицензии, источники, запрещённые зависимости
```

## Параллельные агенты и target

`CARGO_TARGET_DIR=target/agent-<имя>`. Удаляй каталоги завершённых агентов.

## Фикстуры

Тесты сканера работают только на **синтетических деревьях во временных каталогах** (`tempfile`), никогда на реальных данных пользователя. Крупные деревья (1M файлов) генерирует `cargo xtask fixtures` (появится в M1; генератор детерминирован по seed). Корпуса для MFT: образы NTFS с синтетическим содержимым, собираются скриптом из `xtask`, не коммитятся (`corpus/` в `.gitignore`).

## Запуск и управление приложением (с M2)

```sh
cargo run --release -p dirustat -- --control 7981 --scan <тестовая-папка>
```

Control-канал: JSON-строки на `127.0.0.1:7981`, ключ окна лежит в `%APPDATA%\dirustat\control-key.local-7981`. Методы и примеры — `docs/control-protocol.md`. Скриншот: `ui.screenshot` → PNG → посмотри.

Headless-режим CLI:

```sh
cargo run -p dirustat-cli -- scan <папка> --json out.json
cargo run -p dirustat-cli -- run --scan <папка> --cmd export.csv={"path":"out.csv"}
cargo run -p dirustat-cli -- commands            # список команд с описанием параметров
cargo run -p dirustat-cli -- mcp                 # MCP-сервер (stdio)
```

## Отладка

- Логи: `RUST_LOG=dirustat=debug` (крейт `tracing`, подключается в M1). Пользовательский текст ошибок — по-русски, технические теги — по-английски.
- Паника в тесте: `RUST_BACKTRACE=1`. Паника вне тестов — это баг уровня «никогда не падать», заводится регрессионный тест.
- Профилирование: `cargo xtask perf` (M1+) пишет числа в `perf/`; сравнение с бюджетами — `perf/budgets.toml`.
- Windows: `Start-Process -Verb RunAs` для проверки MFT-пути с правами администратора; тесты, требующие админа, помечены `#[ignore]` и запускаются явно (`-- --ignored`).

## Оболочка (Windows)

Репозиторий разрабатывается на Windows. В bash-подобной оболочке длинные heredoc могут ломаться — используй инструмент записи файлов. Пути в коде — через `std::path`, не склеивай строками; тесты не должны зависеть от разделителя.
