# 0001 — Фундамент: цели, инварианты, процессы, стек, плагины, IPC

- **Статус:** согласовано (этап 1 из 7), 2026-11-17
- **Проект:** нативный Wayland-шелл на Rust, целевые композиторы: Hyprland + Niri
- **Репозиторий:** `~/shell` (подключается в `~/dotfiles` как flake input)
- **Следующий документ:** `0002-config-model.md`

## Зафиксированные решения заказчика

| # | Решение | Следствие |
|---|---|---|
| A | Rust, native | нет Qt/GTK/QML; свой рендер, layout, текст |
| B | Полный шелл | бар, панели, OSD, лаунчер, док, дашборд, лок, idle, обои, клипборд, нотификации, трей, скриншот, настройки |
| C | Hyprland + Niri с первого дня | композитор — абстракция с первого коммита |
| D | Плагины на Rust, **in-process dylib** (`abi_stable`/`stabby`) | минимальный ABI-контракт, см. §6 |
| E | Nix + GUI-оверрайды, live-apply, коммит в Nix + rebuild | трёхслойный конфиг + проекция, см. документ 0002 |
| F | matugen-совместимая темизация + шаблоны (kitty, hyprland, niri, …) | подсистема `shell-theme` |
| G | 11 плагинов переезжают с изменённой логикой | capability-модель + хост-API для железа |
| H | OCR-перевод, веб-поиск, ассистент | подсистема `shell-ai` с tool-registry |
| — | Процессы: `shelld` + `shell-ui` + `shell-lock` | см. §4 |
| — | Рендер: `wgpu` + `lyon` + `cosmic-text`/`glyphon` | см. §5 |
| — | MVP v0.1: демон + композиторный слой + бар на Hyprland, Niri вторым | см. §9 |

## 1. Цели

1. Один набор Rust-бинарей, ноль Qt/GTK/Python в рантайме.
2. Полный шелл, но каждая поверхность опциональна (можно запустить только бар).
3. Один код на Hyprland и Niri; новый композитор = новый адаптер, не форк.
4. Единственный источник правды о настройках — Nix; GUI правит на лету и умеет закоммитить в Nix.
5. Плагины на Rust, изолированные по влиянию: падение плагина не уносит сервисы и не раскрывает сессию.
6. Тема из обоев → палитра → шаблоны в kitty/hyprland/niri/GTK/Qt и т.д.
7. AI-фичи (OCR-перевод, поиск, ассистент с вызовом шелла) — первоклассная подсистема, а не curl-скрипт.

## 2. Не-цели

- Не пишем свой композитор/WM, не управляем окнами.
- Не пишем свои приложения (файловый менеджер, терминал, редактор) — только интеграция.
- Нет X11-режима (только XWayland-клиенты через композитор).
- Нет Windows/macOS.
- Не реализуем D-Bus-бэкенды порталов: берём `xdg-desktop-portal-hyprland` / `xdg-desktop-portal-gnome` (Niri), только потребляем.
- GTK3-темы — только как генерируемые файлы, не как рантайм-зависимость.

## 3. Инварианты

- **INV1.** UI не держит системные сервисы. Ни один D-Bus-сервис не живёт в процессе, который рисует бар.
- **INV2.** Композитор — источник истины. Шелл хранит только проекцию состояния.
- **INV3.** Нет настроек «только в GUI»: любой ключ доступен и в Nix-модуле, и в GUI.
- **INV4.** Всё IPC — schema-first: типы в одном крейте, версионируются, дают JSON Schema.
- **INV5.** Ничего не блокирует event loop.
- **INV6.** Плагин не может уронить сервисы и не участвует в экране блокировки.
- **INV7.** Лок-скрин изолирован: падение UI не раскрывает сессию.
- **INV8.** Никаких зависимостей на конкретный композитор вне `shell-compositor/src/<compositor>/`. `grep -r hyprctl crates/` находит совпадения только там.

## 4. Процессная модель

```
  compositor (Hyprland IPC / niri msg) ──► ┌────────────────────────────────┐
                                          │ shelld (tokio, не Wayland)      │
  D-Bus: Notifications, SNI, Polkit,      │  • compositor adapters          │
  MPRIS, logind, NM, BlueZ, UPower        │  • сервисы + владение именами   │
                                          │  • config engine (3 слоя)       │
                                          │  • theme engine                 │
                                          │  • plugin host + AI             │
                                          └────┬──────────────────┬────────┘
                                               │ unix sock        │ unix sock
                                    ┌──────────▼───────┐  ┌───────▼─────────┐
                                    │ shell-ui         │  │ shell-lock      │
                                    │ calloop + wgpu   │  │ calloop, ESL,   │
                                    │ layer-shell      │  │ PAM, без плагинов│
                                    └──────────┬───────┘  └─────────────────┘
                                               │ in-process dylib
                                    ┌──────────▼───────┐
                                    │ plugin .so       │
                                    └──────────────────┘
```

- `shelld` — не Wayland-клиент: только IPC композитора + D-Bus + IPC с UI.
- `shell-ui` — владеет всеми layer-shell поверхностями; рестартуемый (~50–100 мс), сервисы при этом живы.
- `shell-lock` — отдельный бинарь: `ext-session-lock-v1`, PAM, минимум кода, **плагины не грузятся никогда**.
- `shellctl` — внешний клиент того же сокета.

## 5. Стек

| Слой | Выбор | Почему |
|---|---|---|
| Wayland-клиент | `wayland-client` + `smithay-client-toolkit` | пул буферов, seats, output hotplug |
| Session lock, screencopy | raw `wayland-protocols` (`ext-session-lock-v1`, `zwlr_screencopy_v1`, позже `ext-image-copy-capture-v1`) | sctk не обёртывает ESL |
| Event loop (UI) | `calloop` | обязателен для sctk, детерминированный порядок |
| Async (демон) | `tokio` | `zbus`, HTTP, subprocess |
| Рендер | `wgpu` + `lyon` + `cosmic-text`/`glyphon` | GPU-эффекты (blur, скругления, тени) |
| Текст | `cosmic-text`, `fontdb`, `rustybuzz` | shaping, fallback, эмодзи |
| D-Bus | `zbus` | async, без C |
| Аудио | `libpulse-binding` + `pipewire` (спектр) | микшер + визуализатор |
| Сеть/BT/питание | `zbus` к NM / BlueZ / UPower / logind / ppd | события вместо поллинга |
| Клипборд | `wl-clipboard-rs` (`zwlr_data_control_v1`) | фоновый доступ |
| PAM | `pam-client`/`pam-sys`, линковка к системной libpam | см. риск R3 |
| Изображения/палитра | `image`, `fast_image_resize`, `quantette`, `material-colors` (или своя HCT) | один бинарь; внешний `matugen` — опциональный режим |
| Конфиг | `serde` + `toml` + `schemars` | схема для GUI, валидатора, Nix-проекции |
| IPC внутренний | `postcard` (length-delimited) | компактные снапшоты |
| IPC внешний/плагины | **JSON-RPC 2.0** по unix socket | дебаг через `socat`, SDK на любом языке |
| CLI | `clap` (`shellctl`) | — |

## 6. Модель плагинов: in-process dylib с минимальным ABI

Выбрано `abi_stable`/`stabby`, а не процесс. Ниже — как из этого не получить катастрофу.

### 6.1 ABI-поверхность намеренно крошечная

Не пытаемся делать `StableAbi` на всех внутренних типах. Граница ABI — 4 функции и байтовый буфер:

```rust
// shell-plugin-api (публичный)
#[sabi_trait]
pub trait Plugin {
    fn manifest(&self) -> RStr<'_>;                       // TOML-манифест
    fn init(&self, host: &HostApi, cfg: &[u8]) -> RResult<(), RStr<'_>>;
    fn call(&self, method: u32, payload: &[u8]) -> RResult<RVec<u8>, RStr<'_>>;
    fn shutdown(&self) -> RResult<(), RStr<'_>>;
}
```

- `payload`/ответы — `postcard`-сериализованные типы из `shell-proto` (общие с IPC). Внутренние типы свободно эволюционируют, ABI не трогается.
- `HostApi` (`#[sabi_trait]`) — только то, что реально нужно: `log`, `request(capability, method, payload)`, `spawn_task`, `ui_update`, `dbus_call`.
- Цена: плагин не строит нативные виджеты напрямую, а отдаёт **декларативное UI-дерево** (`Row/Column/Text/Icon/Gauge/List/Button/Canvas`), хост рендерит и присылает события. `Canvas` — RGBA-буфер для `cat`/спектра.

### 6.2 Паники и падения

- Каждая ABI-функция возвращает `RResult` — `abi_stable` ловит панику на границе и превращает в `Err`. Паника в плагине не убивает `shelld`.
- Segfault/abort плагина **не ловится**: уносит процесс, в который загружен. Следствия:
  - плагины грузятся **только** в `shelld` и `shell-ui`, никогда в `shell-lock`;
  - оба процесса работают под systemd `Restart=always` + `Type=notify`;
  - `shell-ui` при рестарте пересоздаёт поверхности, сервисы живы (INV1);
  - плагин, упавший 3 раза за 5 минут, автоматически переводится в `disabled` с записью в лог и бейджем в настройках.
- Проверка `plugin_api: u32` + версия `abi_stable` при загрузке; несовпадение — внятная ошибка, не загрузка.

### 6.3 Capabilities: честно

Нативный dylib может вызвать `std::fs` напрямую — **внутрипроцессные capabilities не являются песочницей**, их нельзя принудительно ограничить. Поэтому:

- `plugin.toml` объявляет `capabilities = ["hidraw", "bluetooth", "exec:ydotool", "net:api.anthropic.com", ...]`;
- capability = **декларация + UX-контроль**: что запрошено, показывается при включении плагина, что не объявлено — хост-API не даёт;
- для плагинов, которым нельзя доверять, в манифесте доступен `sandbox = "process"` — тот же `Plugin::call`-протокол (он байтовый), но через дочерний процесс. Транспорт подменяем без изменения API;
- в документации проекта прямо пишем: in-process плагин = доверенный код (как у Noctalia/DMS), `sandbox = "process"` — для остальных.

### 6.4 Версионирование

`plugin_api` инкрементится при любом несовместимом изменении `Plugin`/`HostApi`. Rust-версия компилятора не обязана совпадать (это свойство `abi_stable`), но `abi_stable` major должен совпадать — проверяется при загрузке.

## 7. Cargo workspace

```
shell/
├── crates/
│   ├── shell-proto/        # IPC, плагин-сообщения, UI-дерево, схемы (serde + schemars)
│   ├── shell-config/       # слои, слияние, миграции, валидация, Nix-проекция
│   ├── shell-compositor/   # trait Compositor + hyprland/ + niri/
│   ├── shell-services/     # notifications, tray, audio, net, bt, power, brightness, clipboard, idle, mpris, polkit
│   ├── shell-theme/        # палитра, Material-3, шаблоны, применение
│   ├── shell-plugin-host/  # загрузка dylib, супервизор, capabilities, circuit breaker
│   ├── shell-plugin-api/   # публичный SDK автора плагина
│   ├── shell-ai/           # LLM-клиенты, tool-registry, OCR, перевод, поиск
│   ├── shell-ipc/          # unix-socket сервер, роутер, авторизация клиентов
│   ├── shell-tokens/       # дизайн-токены, цвета, типографика
│   └── shell-ui/           # scene graph, layout, виджеты, wgpu-рендер, поверхности
├── bin/{shelld,shell-ui-bin,shell-lock,shellctl}/
└── flake.nix, modules/{nixos,home-manager}/
```

Правила зависимостей (проверяются в CI тестом на `cargo tree`):

- `shell-proto` не зависит ни от чего внутри.
- `shell-ui` зависит только от `shell-proto`, `shell-tokens`, `shell-plugin-api` — **никогда** от `shell-services`/`shell-config` (всё через IPC от `shelld`).
- `shell-lock` не зависит от `shell-plugin-*` и `shell-ai`.
- `shelld` зависит от всего, кроме рендера UI.

## 8. IPC-контракт v0

- Один сокет на display: `$XDG_RUNTIME_DIR/shell-$WAYLAND_DISPLAY.sock`; JSON-RPC 2.0.
- Модель данных: **snapshot + delta** (эталон — `niri msg event-stream`: полное состояние, затем дельты).
- Топики: `compositor.workspaces|windows|outputs`, `notifications`, `tray`, `audio`, `network`, `bluetooth`, `power`, `mpris`, `clipboard`, `theme`, `config`, `plugins`, `ai`.
- Команды v0: `config.get|set|commit|reload`, `theme.apply|preview`, `compositor.dispatch`, `bar.set_layout`, `panel.toggle`, `osd.show`, `notify.send|close|dnd`, `clipboard.list|copy`, `launcher.query`, `wallpaper.set`, `session.{lock,logout,suspend,reboot,poweroff}`, `audio.{set_volume,toggle_mute,set_default}`, `network.connect`, `bt.pair`, `plugin.{list,enable,disable,reload}`, `ai.request|cancel`, `screenshot.capture`.
- Авторизация: `shellctl` — по uid; плагины — uid + идентификатор плагина.

## 9. MVP v0.1

Демон (`shelld`: конфиг + композиторный слой Hyprland + IPC) → `shell-ui` с баром и OSD. Niri-адаптер подключается сразу после живого Hyprland и обязан не потребовать правок ни в одном крейте, кроме `shell-compositor/src/niri/`.

## 10. Риски и спайки (этап 0, до основного кода)

| # | Риск | Спайк |
|---|---|---|
| R1 | `ext-session-lock-v1` в Rust без обёрток | 1 д. прототип: лок/анлок, 2 монитора |
| R2 | wgpu blur/скругления на 4K@144 | 1 д.: двухпроходный blur, замер |
| R3 | **PAM на NixOS**: у Noctalia helper из чужого кэша ломает анлок. Нам нельзя зависеть от внешнего PAM-хелпера | 0.5 д.: сборка с системным `pam`, лок без override |
| R4 | Яркость внешних мониторов (`ddcutil`/i2c) | 0.5 д.: замер, отдельный поток + кэш |
| R5 | `ext-image-copy-capture-v1` может отсутствовать в `wayland-protocols` | 0.5 д.: vendored XML |
| R6 | Niri без runtime-хоткеев | 1 д.: генератор KDL-binds + `niri validate` |
| R7 | Опасность segfault в dylib-плагине | 0.5 д.: краш-тест (паника, abort, OOM), проверка авто-restart и auto-disable |

## 11. Что заведомо будет не как у них

- **Трей (SNI + dbusmenu)** — самая дорогая позиция проекта; Wine/Electron-приложения не обещаем.
- **NixOS + PAM** — потребует явной настройки (R3).
- **Хоткеи на Niri** — нет runtime-регистрации: генерируем binds-фрагмент композитора (стыкуется с документом 0002).
- **`hyprctl` в общем коде** запрещён (INV8); Hyprland-на-Lua (`hl.dsp.*`) учитывается в адаптере.
