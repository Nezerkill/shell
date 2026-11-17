# 0006 — Тема, плагины, API

- **Статус:** согласовано (этап 6 из 7), 2026-11-17
- **Предыдущий:** `0005-ui.md` · **Следующий:** `0007-ai-packaging-roadmap.md`
- **Решения этапа:** свой индекс файлов (inotify + SQLite); свой шаблонизатор с matugen-совместимым диалектом (+ sidecar-режим); цели темизации v1 = kitty, hyprland, niri, GTK3, Qt; публичный D-Bus `ru.nezerkill.Shell1`; **плагины портируются по одному, поведение каждого согласуется отдельно перед портом**.

## 1. Тема

```
обои → ресайз → квантование (quantette) → seed-цвет
     → HCT-схема → токены (shell-tokens) → UI
     → шаблоны → файлы целей → сигналы reload
```

- Схемы: `tonal-spot` (по умолчанию), `expressive`, `fidelity`, `neutral`, `monochrome`; источник: `wallpaper | seed | static`; light/dark, уровень контраста.
- Шаблонизатор: свой, совместимый с обоими диалектами matugen (`{{colors.primary.default.hex}}`, `{{ primary.hex }}`), фильтры `lighten/darken/alpha/mix`, блоки light/dark.
- `theme.backend = "native" | "sidecar"`: `sidecar` вызывает настоящий `matugen` для капризных целей (Discord и т.п.). По умолчанию на него ничего не завязано.
- Применение: пишем в `~/.config/shell/generated/<target>` → сигнал цели (`SIGUSR1` kitty, reload Hyprland/Niri, `settings.ini` GTK3, `qt6ct` colors).
- Пишем **только свои файлы**; ваши kitty.conf / hyprland.lua / niri.kdl получают один `include`.

## 2. Порты плагинов

Порядок и поведение каждого плагина **согласуются индивидуально перед портом** (решение заказчика). Ниже — заготовка «что плагину нужно от хоста».

| Плагин | Capabilities | Набросок новой роли |
|---|---|---|
| `ajazz-mouse` | `hidraw` (хост-API) | батарея, DPI, профили; дескриптор даёт хост, плагин не трогает `/dev` |
| `honor-earbuds` | `bluetooth` | батарея наушников/кейса, ANC через свойства BlueZ |
| `ai-limits` | `net`, секреты AI | квоты провайдеров через `shell-ai` |
| `drive-health` | `exec:smartctl` | SMART в виджете/панели; привилегии — существующий systemd-юнит |
| `localsend` | `net:lan`, `fs:~/Downloads` | приём/отдача файлов, прогресс, тосты |
| `nix-monitor` | `exec:nix*` | store, поколения, мусор |
| `pomodoro` | — | таймер + OSD + DND |
| `cat` | — (`Canvas`-нода) | анимация в баре/десктопе |
| `bitwarden` | `exec:bw`, секрет сессии | провайдер лаунчера + виджет, авто-lock |
| `autoclicker` | `exec:ydotool` / `uinput` | авто-клик с индикатором; выключение при локе/потере фокуса |

## 3. API: три уровня

| Уровень | Потребитель | Форма |
|---|---|---|
| `shell-plugin-api` | авторы плагинов | `#[sabi_trait] Plugin`, байтовый `call`, `HostApi` |
| IPC / `shellctl` | бинды, скрипты | JSON-RPC, ~25 команд v0 |
| **D-Bus `ru.nezerkill.Shell1`** | любой софт, Stream Deck, Fcitx | `Panels`, `Launcher`, `Notifications`, `Config`, `Ai`, `Theme` |

D-Bus-спека публикуется и версионируется; совместимость в пределах мажора — обещание.

## 4. Файловый поиск

- Свой индексер: обход настраиваемых корней, `inotify` (рекурсивные watch), SQLite (FTS5 для имён), приоритеты и исключения (`.git`, `node_modules`, Nix store по умолчанию).
- Первичный обход — в фоне с прогрессом; поиск не блокирует лаунчер (результаты из памяти + индекс).
- Лимиты: размер БД, максимальная глубина, TTL для мёртвых путей.

## 5. Developer experience

- `shellctl doctor`: D-Bus-имена и их владельцы, версии композитора/порталов, отсутствующие capabilities, битые конфиги, состояние индексера.
- Отладочный оверлей (`SHELL_DEBUG=1`): дерево сцены, границы поверхностей, время кадра, источник данных виджета.
- `tracing` + файл + `SHELL_LOG=<filter>`; запись/replay событий композитора для багрепортов.
- `cargo generate shell-plugin` — шаблон плагина, собирается и грузится за минуту.
