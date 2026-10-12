---
description: Добавить команду в реестр engine по чеклисту проекта
argument-hint: <id команды, например scan.cancel>
---

Добавь команду `$ARGUMENTS` строго по чеклисту «добавить команду» из `docs/contributing.md`:

1. Проверь `docs/target-app-parity.md` и `docs/control-protocol.md` — id и группа должны соответствовать соглашениям.
2. Новый модуль `crates/engine/src/cmd/<area>.rs` (или дополни существующий) с `specs()`; параметры валидируй, ошибки — `CommandError`, без паник и индексации.
3. Деструктивная команда? Тогда `docs/safety.md`: `confirm`, `dry_run`, защищённые пути, журнал, тесты на отказ.
4. Тесты: успех, граничные/мусорные параметры → `Err`; включи команду в hostile-тест.
5. Если есть пункт меню/шорткат — добавь в `crates/ui/src/menu_catalog.rs`.
6. `cargo test -p dirustat-engine`, `cargo xtask ci`.
7. Обнови паритет/gaps и devlog.
