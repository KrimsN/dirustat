## Что и зачем

<!-- Кратко. Ссылка на gap из docs/gaps.md (например G-002) и веху. -->

## Проверка

- [ ] `cargo xtask ci` зелёный (fmt, clippy -D warnings, test, layers, wasm-core)
- [ ] Новый код без `unwrap`/`expect`/`panic!`/индексации/`unsafe` (кроме `dirustat-ntfs`)
- [ ] Тесты добавлены; для фикса падения — регрессионный тест
- [ ] Для новых команд: чеклист из `docs/contributing.md`, hostile-тест
- [ ] Деструктивные операции соответствуют `docs/safety.md`; тесты не трогают реальные данные
- [ ] UI-изменения: скриншот приложен (обе темы)
- [ ] Производительность: числа до/после в `log/devlog.md` (если трогал горячий путь)
- [ ] Clean-room: никакого кода/ресурсов WinDirStat/WizTree; ассеты в `ATTRIBUTION.md`
- [ ] Документы прогресса обновлены (`docs/progress-docs.md`): паритет, gaps, ROADMAP, devlog, STATUS
