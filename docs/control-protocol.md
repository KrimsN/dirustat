# Control-протокол и MCP

> Статус: 2026-10-13 · ревизия 1 · **проект протокола** (реализация — веха M2). `PROTOCOL_VERSION = 1`.

Цель: любое действие человека доступно агенту; приложение можно запустить, просканировать тестовую папку, нажать команды и снять скриншот без мыши и клавиатуры.

## Транспорт

- Запуск: `dirustat --control <порт>`. Слушает только `127.0.0.1:<порт>`.
- Ключ окна генерируется при старте и записывается в файл `control-key.local-<порт>` (Windows: `%APPDATA%\dirustat\`, Linux: `~/.config/dirustat/`, macOS: `~/Library/Application Support/diRUSTat/`). Каждый запрос обязан содержать `key`.
- Формат: одна JSON-строка на запрос, одна на ответ.

```json
{"id":1,"key":"…","method":"engine.execute","params":{"command":"scan.start","params":{"path":"C:\\fixtures\\small"}}}
{"id":1,"ok":true,"result":{"revision":3}}
{"id":2,"ok":false,"error":{"code":"invalid_params","message":"path is empty"}}
```

Коды ошибок (стабильные, английские): `unknown_command`, `invalid_params`, `unsupported`, `confirmation_required`, `not_enabled`, `internal`.

## Методы

| Метод | Параметры | Результат |
|---|---|---|
| `engine.commands` | — | список команд: id, label, menu, shortcut, params_doc, destructive |
| `engine.execute` | `command`, `params` | результат команды; деструктивные требуют `confirm: true` |
| `session.state` | `path?` | JSON состояния сессии (дерево верхнего уровня, выбор, прогресс, ревизия) |
| `session.tree` | `node?`, `depth?`, `limit?` | поддерево с размерами (пагинация, без полного дерева) |
| `ui.state` | — | сериализованное состояние UI (serde) |
| `ui.set_state` | `patch` | изменить состояние UI (панели, сортировка, масштаб) |
| `ui.screenshot` | `path`, `region?` | сохранить PNG окна (рендер offscreen, без захвата фокуса) |
| `app.quit` | — | корректное завершение |

Правила:
- Программный вызов никогда не открывает диалог; всё, что в UI требует диалога, имеет параметризованный вариант команды.
- `engine.execute` блокирует до завершения быстрых команд; долгие (`scan.start`) возвращают сразу, прогресс — через `session.state`.
- Ответы ограничены по размеру: большие деревья отдаются страницами (`session.tree`).

## MCP

`dirustat-cli mcp` — MCP-сервер (stdio), `dirustat-cli mcp --connect 127.0.0.1:<порт>` — мост к запущенному приложению. Каждая команда реестра публикуется как tool с JSON-схемой параметров; деструктивные помечены `destructive: true` и требуют `confirm`. Дополнительно: ресурсы `dirustat://session/state`, `dirustat://session/tree`.

## CLI-эквиваленты

```sh
dirustat-cli commands
dirustat-cli scan <папка> --json out.json
dirustat-cli run --scan <папка> --cmd 'export.csv={"path":"out.csv"}'
```

## Соглашения о командах

- id: `группа.действие` (`scan.start`, `scan.cancel`, `view.zoom_in`, `select.node`, `file.delete`, `export.csv`, `search.find`).
- Группы: `scan`, `view`, `select`, `tree`, `treemap`, `ext`, `file`, `cleanup`, `search`, `filter`, `export`, `snapshot`, `settings`, `ui` (только оболочка).
- Параметры — объект JSON; все поля документированы в `params_doc`; неизвестные поля — ошибка `invalid_params`.

## Безопасность канала

Только loopback, обязательный ключ, лимит размера запроса, лимит скорости; деструктивные команды — по `docs/safety.md`. Канал по умолчанию выключен.
