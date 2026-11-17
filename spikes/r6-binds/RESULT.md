# R6 — генератор KDL-binds для Niri

Спайк: `spikes/r6-binds` (свой crate, без зависимостей); воспроизводит `./run.sh`
(niri 26.04, 29 PASS, exit 0). Критерий выполнен.

## Вердикт

**Идём.** Генератор из спека в KDL-фрагмент работает, `niri validate` его принимает
(exit 0, `INFO niri: config is valid`), а на заведомо битом фрагменте честно падает
(exit 1, `Error:`) — проверка реальная, не всегда-зелёная.

Факты ниже — из `niri validate` 26.04, не из документации:

- `binds { … }` — **только верхний уровень**; внутри `input {}` → `unexpected node`.
- Комбо — `модификаторы+XKB-keysym`. Принимаются `Mod`, `Ctrl`/`Control`, `Alt`,
  `Shift`, `Super`, `Win`, `ISO_Level3_Shift`, `ISO_Level5_Shift`; `Meta`, `Hyper`,
  `Logo`, `Mod4`, `Caps_Lock`, `AltGr` — отказ (`invalid modifier`).
- `Super` — **не синоним** `Mod`: `Mod+Return` и `Super+Return` — разные бинды
  (оба валидны), т.е. `SUPER → Mod` — наше решение; дефолт niri пишет, что `Mod` =
  Super на TTY.
- Порядок модификаторов не важен: `Mod+Shift+Q` и `Shift+Mod+Q` — один бинд
  (`duplicate keybind later defined here`). Но **дубликат через `include` не
  ловится** (exit 0) — конфликты между файлами шелл ищет сам.
- Один экшен на бинд (`only one action is allowed per keybind`), узел закрывается
  `;`. Аргументы: `spawn "foot"`, `spawn "prog" "arg"`, `spawn-sh "строка"`,
  целое — без кавычек (`focus-workspace 2`), строка — в кавычках
  (`set-column-width "+10%"`; `+10%` без кавычек — ошибка парсинга).
  Неизвестный экшен — `expected quit, suspend, or one of 133 others`.
- Свойства: `repeat=false`, `cooldown-ms=150`, `hotkey-overlay-title="текст"|null`,
  `allow-when-locked=true` — **только** на `spawn`/`spawn-sh`, иначе конфиг не грузится.
- `hotkey-overlay { skip-at-startup }` — отдельный верхнеуровневый блок (внутри
  `binds` → `invalid key: hotkey-overlay`).
- `include "path"` резолвится от файла-включателя; отсутствующий include — ошибка.

## Как проверял

`run.sh` ре-экзек в `nix develop`, достаёт niri через
`nix build --no-link --print-out-paths nixpkgs#niri`, без сети печатает `SKIP` и
выходит с 0. Генератор — `r6-binds gen <spec> <out.kdl>`, 13 unit-тестов,
`cargo build --offline` (в кэше реестра нет serde_json, JSON-парсер свой).
Спека — карта `binds.<id> = { keys, do, … }` (как `binds: { action = "SUPER+b" }`
в `0003-compositor.md`), `do.type` ∈ `spawn`/`spawn-sh`/`niri`/`ipc`, IPC =
`spawn "shellctl" "call" "<call>"`. Вызовы niri — с `env -u …`: сессия не нужна.

## Доказательства

```
[1] minimal.kdl                rc=0  config is valid
[2] full.kdl (10 биндов)       rc=0  config is valid
[3] bad-action.kdl (spawns)    rc=1  expected `quit`, `suspend`, or one of 133 others
[4] fixtures/duplicate.kdl     rc=1  duplicate keybind later defined here
[5] спека с дублем             rc=2  duplicate keybind `Mod+Shift+Q` (порядок нормализован)
[6] allow_when_locked на action rc=2 отказано до записи файла
[7] дефолт из бинаря niri      rc=0  632 строки, взяты как эталон синтаксиса
[8] include "shell-binds.kdl"  rc=0
[9] дубль через include        rc=0  ← дыра, закрывается только своим детектором
RESULT: all assertions passed (29 PASS)
```

## Что записать в docs/0003-compositor.md

- Фрагмент шелла — **отдельный файл** `shell-binds.kdl`, подключаемый через
  `include` (относительный путь от `config.kdl`); `binds` в нём — верхнеуровневый.
- Генератор обязан писать `Mod` (а не `Super`) для нашего `SUPER`; `Super` в KDL —
  другой бинд.
- Валидация перед записью: `niri validate -c <файл>`, критерий — exit 0 и
  отсутствие `Error:` в stderr; сессия не нужна, годится для CI.
- Детект конфликтов — **свой**: niri не видит дубликаты между `include`-файлами.
  Сравнивать надо канонический набор модификаторов (порядок не значим), иначе
  `SHIFT+SUPER+Q` проскочит мимо `SUPER+SHIFT+Q`.
- `allow-when-locked` — только для `spawn`/`spawn-sh`; проверять в генераторе.
- `hotkey-overlay { skip-at-startup }` — лишь если блока ещё нет у пользователя.

## Что не проверено

- Поведение биндов в живой сессии: доставка нажатий, приоритет при дубликате через
  `include`, реальное `Mod` = Super на этом хосте (это комментарий из дефолта niri).
- Полный список 135 экшенов и семантика аргументов (`focus-workspace "2"` как имя
  воркспейса, а не индекс).
- Рендер `hotkey-overlay`, `cooldown-ms` в рантайме.
- Другие версии niri: всё снято на 26.04.
