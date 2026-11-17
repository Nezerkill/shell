# 0003 — Композиторный слой: Hyprland + Niri

- **Статус:** согласовано (этап 3 из 7), 2026-11-17; решения проверены спайками R1/R5/R6 (2026-11-17)
- **Предыдущий:** `0002-config-model.md` · **Следующий:** `0004-services.md`
- **Решения этапа:** свой overview для Hyprland (через toplevel-export); хоткеи — генератор фрагментов композитора + `include`; шелл пишет только **свои** файлы-фрагменты; композитор-специфичные действия показываются через capabilities; недоступное — **скрывается**.

## 1. Форма абстракции

```rust
pub trait CompositorBackend: Send {
    fn capabilities(&self) -> Capabilities;
    fn snapshot(&self) -> Snapshot;
    fn subscribe(&self, tx: Sender<RawEvent>) -> Subscription;
    fn dispatch(&self, action: Action) -> Result<(), DispatchError>;
    fn outputs_mut(&self) -> Box<dyn OutputControl>;
}
```

Конвейер: `RawEvent` → `Normalizer` → `CompositorState { seq, … }` + `StateDelta`.
Ни один крейт вне `shell-compositor` не разбирает `EVENT>>`-строки и не зовёт `hyprctl`/`niri msg` (INV8, проверяется grep-тестом в CI).

## 2. Модель данных

| Тип | Поля | Замечание |
|---|---|---|
| `OutputId` | непрозрачный: connector + хэш EDID/серийника | `DP-1`/`DP-2` меняются местами на доке |
| `WorkspaceId` | непрозрачный | Niri: позиционный и меняется; Hyprland: числовой/именованный |
| `Workspace` | `id, output, name?, index?, is_active, is_urgent, is_focused` | `index` только для сортировки |
| `Window` | `id, title, app_id, pid, workspace?, is_focused, is_floating, is_fullscreen, is_urgent, geometry?` | — |
| `State` | `outputs, workspaces, windows, focus_window?, focus_output?, kbd_layouts, overview?` | + `seq` |

**Воркспейсы (главная ловушка).** Внутренняя модель = `output → упорядоченный список воркспейсов` + отдельный список именованных; не «массив 1..10». На Hyprland нормализатор добивает пустые слоты до `workspaces = N` из конфига, чтобы вид бара совпадал с ожиданием пользователя. На Niri список динамический, пустые в середине исчезают, всегда есть один пустой в конце — бар рисует ровно то, что в модели.

## 3. Потоки событий и живучесть

| | Hyprland | Niri |
|---|---|---|
| Источник | `.socket2.sock` + `hyprctl -j` для снимка | `$NIRI_SOCKET` `event-stream`: полное состояние, затем дельты |
| Слабое место | события без `seq` → возможны пропуски | дельты «в основном» атомарны: окно может ссылаться на удалённый воркспейс |
| Стратегия | подписка → снимок → применить буфер; расхождение лечится следующим событием | дельты как есть; висячие ссылки вычищает нормализатор (`debug!`) |

Общее:
- `seq` ведёт наш нормализатор; отставший клиент запрашивает полный снапшот.
- Разрыв сокета → exponential backoff, при восстановлении **полный снапшот**.
- Неподтверждённое состояние помечается `stale` (UI не врёт старыми данными).
- Проверка версии композитора при подключении; мажорное расхождение → предупреждение.

## 4. Матрица действий

| Действие | Hyprland | Niri |
|---|---|---|
| spawn | `dispatch exec` | `action spawn -- sh -c` |
| close / focus окна | `killactive`, `focuswindow address:` | `close-window`, `focus-window --id` |
| focus / move to workspace | `workspace`, `movetoworkspace` | `focus-workspace`, `move-window-to-workspace` |
| переименовать воркспейс | `renameworkspace` | `set-workspace-name` |
| overview | **нет** → наш (см. §7) | `toggle-overview` + `overview-state` |
| скриншот | grim/portal | `action screenshot*` |
| пипетка | `hyprpicker` (внешний) | `action pick-color` |
| экраны off | `dispatch dpms off` | `action power-off-monitors` |
| мониторы | `keyword monitor` | `niri msg output` / `wlr-output-management` |
| float / fullscreen | `togglefloating`, `fullscreen` | `toggle-window-floating`, `fullscreen-window` |
| колонки/consume/expand | — | есть |
| special workspace / pin / pseudo | есть | — |
| правила окон | `setprop` + конфиг | только KDL, reload |
| runtime-хоткеи | `hyprland-global-shortcuts-v1` | **нет** |

## 5. Capabilities

```rust
bitflags! { pub struct Capabilities: u32 {
    const OVERVIEW = 1<<0;          const NAMED_WORKSPACES = 1<<1;
    const RUNTIME_HOTKEYS = 1<<2;   const WINDOW_RULES_API = 1<<3;
    const COLUMNS = 1<<4;           const SCRATCHPADS = 1<<5;
    const NATIVE_SCREENSHOT = 1<<6; const OUTPUT_MIRRORING = 1<<7;
    const KBD_LAYOUT_SWITCH = 1<<8; const URGENCY = 1<<9;
    const TOPLEVEL_EXPORT = 1<<10;
}}
```
UI и конфиг **не ветвятся на имя композитора** — только на capabilities. Элемент без capability **скрывается** (решение этапа); в настройках остаётся строка «недоступно на текущем композиторе».

## 6. Хоткеи

- Единый реестр: `binds: { action = "SUPER+b" }` в конфиге шелла.
- Два кодогенератора из одного реестра:
  - **Hyprland**: `shell-binds.lua` (`hl.bind(...)`, т.к. конфиг пользователя — Lua) либо `.conf`-фрагмент, если конфиг не Lua;
  - **Niri**: `shell-binds.kdl`.
- Файлы пользователя не изменяются: генерируем отдельные файлы, пользователь один раз добавляет `require`/`include`.
- Валидация перед записью: `niri validate`; на Hyprland — проверка через `hyprctl configerrors`-эквивалент, при ошибке — откат.
- Детект конфликтов: парсим существующие бинды пользователя на чтение и предупреждаем о пересечениях.

## 7. Overview

| | Реализация |
|---|---|
| Niri | нативный; шелл читает `overview-state` и не рисует свой (только реакция: при открытом overview бар может приглушаться) |
| Hyprland | **свой**: грид превью окон через `hyprland-toplevel-export-v1` (+ fallback screencopy+кроп), клик — фокус, drag — перенос в воркспейс |

Требует собственной подсистемы капчура, поэтому в плане — отдельным спринтом (см. roadmap).

## 8. Скриншоты и капчур

| | Hyprland | Niri |
|---|---|---|
| окно | `hyprland-toplevel-export-v1` | `ext-image-capture-source-v1` / screencopy |
| область/вывод | `zwlr_screencopy_v1` | то же + встроенный UI |
| каст | portal ScreenCast (portal-hyprland) | portal (`-gnome`) |

## 9. Тесты

- Фикстуры снимков и потоков событий обоих композиторов в репозитории; golden-тесты нормализатора.
- **Record/replay**: реальный поток `snapshot + events` → JSONL → проигрывание в CI (регрессии парсеров без композитора).
- **Хаос**: обрыв сокета посреди потока, перезапуск композитора, окно со ссылкой на удалённый воркспейс, отключение монитора — без паник и потери состояния.

## 10. Лок: `ext-session-lock-v1` (R1)

Источник — спайк R1 (headless sway 1.12 / wlroots 0.20.2, 75 ассертов); обязательное чтение для `shell-lock`.

- Менеджер только **v1**: `lock`, `get_lock_surface`, `ack_configure`, `unlock_and_destroy`; события `locked` | `finished`, по одному на объект.
- `locked` приходит **до** любого `lock_surface`: выходы закрывает сам композитор (у sway — чёрным). Жизненный цикл поверхности ведёт пара `locked` + `new_surface`, а не «лок = кадр на экране».
- Ровно один `lock_surface` на `wl_output`; повторный `get_lock_surface` на тот же выход — `duplicate_output`.
- Каждый `configure` подтверждается `ack_configure(serial)` до коммита, иначе ошибка протокола; буфер ровно w×h из последнего configure, null-буфер — тоже ошибка.
- После `unlock_and_destroy` `wl_surface` уничтожать **нельзя**: role-объект уже снесён композитором (wlroots: `surface was destroyed before its role object`). Уничтожать только role-объект, оставляя поверхность, либо закрывать соединение.
- Второй клиент при живом локе получает `finished`; смерть клиента лок **не** снимает: сессия остаётся залоченной, композитор заливает выходы своим цветом (`session lock abandoned`), а новый клиент получает `locked` с `Replacing abandoned lock` — восстановление после краха на уровне протокола.
- Ресайз/scale → новый `configure`, acked-размер обязан совпасть с буфером до пикселя (проверено 1280x720 → 2560x1440 → 1600x900 → scale 2, 12 перерисовок, ни одного кадра со столом); старый буфер держится до коммита нового.

Всё проверено на wlroots (headless sway) и **не** на Hyprland: политика лока Hyprland не проверена, подтверждено только, что Hyprland 0.56.2 рекламирует менеджер v1.

## 11. Захват экрана: `ext-image-copy-capture-v1` (R5)

Чем снимаем — §8; здесь — на чём это собирать и что проверено.

- `ext-image-copy-capture-v1` и `ext-image-capture-source-v1` лежат в wayland-protocols 1.49 в `staging/`, не в `stable/`; интерфейсы v1, `ext_image_capture_source_v1` помечен `frozen="true"` (правки — только новым major'ом).
- Решение: crates.io `wayland-protocols` **0.32.13** с `features = ["client", "staging"]` — XML в крейте байт-в-байт равны копиям из 1.49. Без `staging` получаем не пустой модуль, а ошибку компиляции (проверено).
- Vendored XML **не берём**: резервный путь собран и работает, но тянет третий файл `ext-foreign-toplevel-list-v1.xml` (из-за `ext_foreign_toplevel_handle_v1`), а `pkg-config --variable=pkgdatadir` отдаёт store-путь с удвоенным ведущим слэшем, который в build-скриптах приходится нормализовать.
- Проверен только `bind` трёх менеджеров на живом Hyprland 0.56.2 — реальной сессии захвата не было (shm/dmabuf, `buffer_constraints`, `stopped` не проверялись). Staging значит, что следующий wayland-protocols может его заменить или убрать `frozen`-гарантии.

## 12. Бинды Niri: KDL-фрагмент (R6)

Факты — из `niri validate` 26.04. Генератор `shell-compositor` пишет `shell-binds.kdl`, пользователь подключает его через `include "shell-binds.kdl"` (путь резолвится от файла-включателя).

- `binds { … }` — только верхний уровень; внутри `input {}` → `unexpected node`.
- `Super` — **не** `Mod`: `Mod+Return` и `Super+Return` разные бинды, поэтому наш `SUPER` генерируется как `Mod`.
- Порядок модификаторов не значим (`Mod+Shift+Q` == `Shift+Mod+Q`), поэтому сравнивать только канонический набор — иначе `SHIFT+SUPER+Q` пройдёт мимо дубликата.
- `allow-when-locked` допустим **только** на `spawn`/`spawn-sh`, иначе конфиг не грузится — проверять в генераторе.
- `hotkey-overlay { skip-at-startup }` — отдельный верхнеуровневый блок (внутри `binds` — `invalid key`); писать, только если блока у пользователя ещё нет.
- Оффлайн-валидатор — `niri validate -c <файл>` (критерий: exit 0 и отсутствие `Error:`, сессия не нужна, годится для CI). Дубликаты **внутри** файла он ловит, между `include`-файлами — нет, поэтому конфликты между файлами шелл ищет сам.
