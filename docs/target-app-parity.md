# Паритет с WinDirStat и WizTree

> Статус: 2026-10-13 · ревизия 1 · WinDirStat 0/34, WizTree 0/12. Поведение определено по пользовательской документации и использованию программ как чёрных ящиков (`docs/clean-room.md`).

Статусы: `missing` · `partial` · `done`. При изменении строки обнови дату, пересчитай итоги в шапке и в `ROADMAP.md`, допиши историю ревизий. Идентификаторы — id команд (`docs/control-protocol.md`).

## A. WinDirStat (34)

| # | Функция | Команда / область | Веха | Статус |
|---|---|---|---|---|
| 1 | Скан выбранной папки | `scan.start` | M1 | missing |
| 2 | Выбор дисков (все локальные / отдельные) | `scan.start_drives` | M1 | missing |
| 3 | Отмена скана | `scan.cancel` | M1 | missing |
| 4 | Повторный скан / обновление | `scan.rescan` | M1 | missing |
| 5 | Индикация прогресса и оценки | статус-бар | M1 | missing |
| 6 | Исключения при скане (маски, пути) | `scan.exclude` | M4 | missing |
| 7 | Раскрытие/сворачивание дерева | `tree.expand` / `tree.collapse` | M1 | missing |
| 8 | Сортировка по колонкам | `tree.sort` | M1 | missing |
| 9 | Колонки: имя, доля (бар), %, размер, элементы, файлы, папки, изменён, атрибуты | `tree.columns` | M1 | missing |
| 10 | Синхронизация выбора дерево ↔ treemap ↔ расширения | `select.node` | M1 | missing |
| 11 | Режим размера: логический / на диске | `settings.size_mode` | M1 | missing |
| 12 | Элементы «свободное место» и «неизвестно» | `view.free_space` | M2 | missing |
| 13 | Treemap squarified | `treemap.render` | M1 | missing |
| 14 | Cushion-шейдинг | `treemap.cushion` | M5 | missing |
| 15 | Параметры treemap (сетка, яркость, высота, масштаб, свет) | `treemap.options` | M5 | missing |
| 16 | Зум в/из каталога | `view.zoom_in` / `view.zoom_out` | M2 | missing |
| 17 | Подсветка выбранного на treemap | `treemap.highlight` | M1 | missing |
| 18 | Выбор кликом по treemap | `treemap.pick` | M1 | missing |
| 19 | Показ/скрытие treemap | `view.toggle_treemap` | M1 | missing |
| 20 | Список типов файлов (расширение, размер, число, %) | `ext.list` | M1 | missing |
| 21 | Цвета типов файлов | `ext.colors` | M1 | missing |
| 22 | Подсветка файлов расширения на treemap | `ext.highlight` | M2 | missing |
| 23 | Сортировка списка расширений | `ext.sort` | M1 | missing |
| 24 | Открыть файл | `file.open` | M4 | missing |
| 25 | Показать в проводнике | `file.reveal` | M4 | missing |
| 26 | Удалить в корзину | `file.delete` | M4 | missing |
| 27 | Удалить безвозвратно | `file.delete_permanent` | M4 | missing |
| 28 | Копировать путь | `file.copy_path` | M4 | missing |
| 29 | Пользовательские очистки (cleanups) | `cleanup.run` | M4 | missing |
| 30 | Свойства файла/папки | `file.properties` | M4 | missing |
| 31 | Поиск по имени | `search.find` | M4 | missing |
| 32 | Фильтр по имени/размеру/дате | `filter.set` | M4 | missing |
| 33 | Сохранение настроек | `settings.save` | M2 | missing |
| 34 | Запуск из командной строки с путём | `dirustat <path>` | M2 | missing |

## B. Отличительные возможности WizTree (12)

| # | Функция | Команда / область | Веха | Статус |
|---|---|---|---|---|
| 1 | Скан через MFT | `scan.start` (backend `mft`) | M3 | missing |
| 2 | Запрос прав администратора и откат на обычный скан | `scan.elevate` | M3 | missing |
| 3 | Плоский список всех файлов | `view.file_list` | M2 | missing |
| 4 | Топ самых больших файлов | `view.largest_files` | M2 | missing |
| 5 | Поиск с wildcard/regex | `search.find` (mode) | M4 | missing |
| 6 | Экспорт CSV | `export.csv` | M5 | missing |
| 7 | Экспорт изображения treemap | `export.image` | M5 | missing |
| 8 | Сохранение/загрузка снимков скана | `snapshot.save` / `snapshot.load` | M5 | missing |
| 9 | Сравнение снимков | `snapshot.compare` | M5 | missing |
| 10 | Выбор диска с полосами заполнения | `view.drive_picker` | M2 | missing |
| 11 | Скан не-NTFS / сетевых путей | `scan.start` (backend `portable`) | M1 | missing |
| 12 | Портативный режим (без установки, настройки рядом) | `settings.portable` | M6 | missing |

## C. Сверх паритета (не входит в проценты)

Дубликаты файлов (хэш по размеру → частичный → полный), история изменений между снимками с графиком, тёмная/высококонтрастная темы, палитра команд, управление агентами (control/MCP), CLI-экспорт, шаблоны очисток, подсказки «что можно удалить» (кэши, временные).

## D. Метрики

| Метрика | Значение |
|---|---|
| WinDirStat, `done` | 0 / 34 |
| WizTree, `done` | 0 / 12 |
| Скорость MFT-скана (1M файлов) | не измерена |

## История ревизий

- 2026-10-13 · r1 — первичная матрица.
