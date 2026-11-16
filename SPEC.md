# Brisk — лёгкая IDE с поведением PyCharm

Рабочее название: **Brisk**. Цель: человек из PyCharm открывает проект и работает теми же руками
(окна, сочетания клавиш, поведение), но IDE стартует меньше чем за секунду и не ест гигабайты.

Фокус: Python first + современный фронтенд (TS/JS, React/Vue, CSS, Tailwind).

---

## 1. Бюджеты (проверяются в CI на каждом коммите)

| Метрика | Цель |
|---|---|
| Холодный старт до открытого проекта | < 1 с |
| Память без проекта | < 80 МБ |
| Память, средний Python+фронтенд проект, вместе с языковыми серверами | < 400 МБ |
| Ввод до символа на экране | < 10 мс |
| Установщик | < 50 МБ |
| Повторное открытие проекта (индекс с диска, mmap) | < 300 мс до работающего Search Everywhere |

---

## 2. Принципы

1. **Rust везде.** Ядро, UI, индекс. Внешние инструменты — тоже по возможности нативные (ruff, ty, tsgo, biome).
2. **Ленивость.** Ничего не запускается и не индексируется, пока не нужно. Зависимости индексируются в фоне.
3. **Процессы — главный источник раздувания.** Каждый внешний сервер стартует лениво, один на проект, гасится при простое.
4. **Виртуализация всего.** Деревья, списки, таблицы, лог рисуют только видимое.
5. **Ядро без UI.** Вся логика в крейтах без зависимости от GUI — тестируется отдельно, UI тонкий.
6. **Совместимость с PyCharm по поведению**, не по внешнему виду: иконки и графика свои (авторское право JetBrains),
   раскладка окон, сочетания клавиш, логика — те же. Импорт keymap XML и чтение `.idea/`.

---

## 3. Архитектура

```
brisk-app            UI (GPUI): окна, панели, редактор, диалоги
  │
  ├─ brisk-workspace   модель окна: открытые файлы, раскладка tool windows, действия
  ├─ brisk-keymap      действия + раскладка PyCharm, аккорды, двойной Shift
  ├─ brisk-text        буфер (rope), правки, undo/redo, кодировки, переводы строк
  ├─ brisk-project     дерево файлов, исключения, watcher, импорт .idea
  ├─ brisk-index       индекс символов (свой, mmap на диске), триграммный индекс для Find in Files
  ├─ brisk-syntax      tree-sitter: подсветка, folding, структура, injections
  ├─ brisk-lsp         клиент LSP + менеджер процессов (ленивый старт, idle shutdown)
  ├─ brisk-dap         клиент DAP (debugpy, CDP для браузера)
  ├─ brisk-python      окружения (venv/uv/poetry/conda/pyenv) без запуска интерпретатора, pytest
  ├─ brisk-git         статус, коммит, ветки, лог-граф (commit-graph), blame, diff (imara-diff), merge
  ├─ brisk-run         run configurations, процессы, вывод, консоль
  ├─ brisk-terminal    терминал (alacritty_terminal) с отрисовкой в нашем UI
  ├─ brisk-docker      Docker Engine API (named pipe / unix socket), compose
  ├─ brisk-db          Postgres/MySQL/SQLite, схема, консоль, грид результатов
  ├─ brisk-history     Local History: content-addressed + zstd
  └─ brisk-remote      маленький агент для WSL/SSH/Docker
```

### Внешние движки

| Задача | Движок | Язык |
|---|---|---|
| Типы, автодополнение, переходы Python | `ty` (резерв: `pyrefly`, `basedpyright`) | Rust |
| Линтер/форматтер Python | `ruff` | Rust |
| Отладка Python | `debugpy` | Python |
| TypeScript | `tsgo` | Go |
| Линт/формат JS/TS/CSS/JSON | `biome` / `oxc` | Rust |
| CSS/HTML/Tailwind | **свои** на tree-sitter + индексе | Rust |
| ESLint, Prettier, Volar | только если проект требует, лениво | Node |

---

## 4. Что пишем сами ради лёгкости

| Компонент | Зачем |
|---|---|
| Индекс символов | Мгновенные Search Everywhere / Go to Symbol / автоимпорт. Для site-packages — только сигнатуры. Интернирование строк, плоские массивы, mmap, кэш по `пакет==версия` |
| Менеджер языковых серверов | Ленивый старт, idle shutdown, лимиты памяти |
| Триграммный индекс | Find in Files мгновенно в монорепо |
| Детект Python-окружений | Чтение `pyvenv.cfg`, `uv.lock`, `poetry.lock`, conda без запуска python; кэш `sys.path` |
| Поиск тестов по индексу | Иконки ▶ у `test_*` сразу, без `pytest --collect-only` |
| Local History | Дедупликация + сжатие |
| Git-лог граф | Своя раскладка, чтение `commit-graph` |
| CSS/HTML/Tailwind | Вместо Node-серверов |
| Дисциплина памяти | Выгрузка буферов и деревьев tree-sitter неактивных файлов, бюджет на подсистему |
| Remote-агент | Маленький бинарник вместо всей IDE на удалённой стороне |

---

## 5. Совместимость с PyCharm (обязательный минимум)

### Окна (Alt+цифра)
| Окно | Сочетание |
|---|---|
| Project | Alt+1 |
| Bookmarks | Alt+2 |
| Find | Alt+3 |
| Run | Alt+4 |
| Debug | Alt+5 |
| Problems | Alt+6 |
| Structure | Alt+7 |
| Services (Docker, БД) | Alt+8 |
| Git | Alt+9 |
| Commit | Alt+0 |
| Terminal | Alt+F12 |
| Database | — (через Services / Search Everywhere) |

### Поведение
- Автосохранение при потере фокуса и переключении файла.
- Двойной Shift — Search Everywhere; Ctrl+Shift+A — Find Action.
- Alt+Enter — quick fixes/intentions; Ctrl+B / Ctrl+клик — переход к определению.
- Live templates и postfix completion (`.if`, `.for`, `.not`, `main`).
- Language injection: SQL в строках Python, HTML/CSS в JS.
- Shelve, changelists, Local History, TODO, breadcrumbs, sticky lines, inlay hints, Ctrl+P.
- Импорт `.idea/` (интерпретатор, run configurations, source roots, исключения) и keymap XML.

Полная таблица сочетаний — `crates/brisk-keymap/src/pycharm.rs`.

---

## 6. Этапы

| # | Этап | Результат |
|---|---|---|
| 1 | Каркас | Окно, tool windows, дерево проекта, редактор с подсветкой, keymap PyCharm, Search Everywhere по файлам, терминал |
| 2 | Python | Индекс символов, ty, ruff, окружения, run configurations, debugpy, pytest |
| 3 | Git | Коммит, ветки, лог-граф, diff, merge, blame, shelve |
| 4 | Фронтенд | tsgo, biome, свои CSS/HTML/Tailwind |
| 5 | Services | Docker, БД |
| 6 | Миграция | Импорт `.idea`, рефакторинги, фреймворки (Django, FastAPI), Remote |

---

## 7. Риски

- **GPUI** быстро меняет API; закрепляем версию, UI держим тонким.
- **Слабое железо без GPU:** на Windows DirectX 11 падает на WARP (программный рендер) — работает, но медленнее.
- **Рефакторинги** уровня PyCharm (change signature, move) — частично свои поверх индекса.
- **Знание фреймворков** (Django ORM, шаблоны) — своя логика, этап 6.
