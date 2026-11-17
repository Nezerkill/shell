# 0003 — Композиторный слой: Hyprland + Niri

- **Статус:** согласовано (этап 3 из 7), 2026-11-17
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
