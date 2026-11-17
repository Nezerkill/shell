# 0004 — Сервисы ОС: владение D-Bus, надзор, лок, трей

- **Статус:** согласовано (этап 4 из 7), 2026-11-17
- **Предыдущий:** `0003-compositor.md` · **Следующий:** `0005-ui.md`
- **Решения этапа:** полный SNI + dbusmenu; сеть через NetworkManager (+ secret agent); свой рендерер обоев; **шифрованная история клипборда**; лок = пароль + fprint через PAM.

## 1. Владение именами

Принцип INV1: все D-Bus-имена держит `shelld`; `shell-ui` только отображает. Рестарт UI не теряет нотификации, трей и polkit-диалоги.

| Сервис | Интерфейс | Владеем | Сложность |
|---|---|---|---|
| Нотификации | `org.freedesktop.Notifications` | да | средняя |
| Трей | `org.kde.StatusNotifierWatcher` + прокси SNI + `com.canonical.dbusmenu` | да | **высокая** |
| Polkit | `org.freedesktop.PolicyKit1.Agent` | да | средняя |
| MPRIS | слушаем `org.mpris.MediaPlayer2.*` | нет | низкая |
| Аудио | PipeWire/Pulse | нет | средняя |
| Сеть | NetworkManager + secret agent | агент — да | **высокая** |
| Bluetooth | BlueZ + `org.bluez.Agent1` | агент — да | **высокая** |
| Питание | UPower + logind + `net.hadess.PowerProfiles` | нет | низкая |
| Яркость | logind `SetBrightness` + sysfs + ddcutil | нет | средняя |
| Idle/DPMS | `ext-idle-notify-v1` + `zwp_idle_inhibit_v1` | да | низкая |
| Лок | `ext-session-lock-v1` (в `shell-lock`) | да | **высокая** |
| Клипборд | `zwlr_data_control_v1` | нет | средняя |
| Обои | свой layer-shell background | да | низкая |
| Скриншот | screencopy / toplevel-export | да | средняя |
| Ночной свет | `zwlr_gamma_control_v1` | да | низкая |
| Сессия | logind | нет | низкая |
| Приватность (mic/cam/share) | PipeWire clients | нет | низкая |
| Системный монитор | `/proc`, sysfs, nvml/amdgpu | нет | средняя |

Порталы не реализуем: `xdg-desktop-portal-hyprland` / `-gnome`; шелл проверяет наличие и предупреждает (`shellctl doctor`).

## 2. Конфликты имён

- Пробуем взять имя; `EXISTS`/`IN_QUEUE` → **не падаем**, лог + бейдж «нотификации обслуживает другой демон».
- `shellctl doctor` — занятые имена с pid владельца.
- В Nix-модуле `services.guard` — отключает mako/swaync/nm-applet при включении шелла.

## 3. Надзор

```
spawn_service!(name) → panic/Err → backoff → restart (счётчик)
                     → 5 падений / 10 мин → disabled + событие в UI + бейдж в настройках
```
Паника в сервисе не убивает процесс и не трогает UI.

## 4. Порядок старта

- `Type=notify`, `After=graphical-session.target networkmanager.service upower.service`.
- Ожидание имён с таймаутом и повтором, никогда `sleep`.
- `PrepareForSleep`: сначала лок → ждём подтверждения от `shell-lock` (кадр отрисован на всех выходах) → отпускаем suspend. Иначе на резюме на кадр виден рабочий стол.
- Единая политика idle/DPMS/лок/suspend в одном месте (не «hypridle + скрипт + хук»).

## 5. Лок-скрин

- Отдельный процесс `shell-lock`: ESL-поверхности, PAM, часы и медиа-мелочь. Плагины не грузятся никогда.
- PAM: линковка к системной `libpam` (риск R3); fprint — если в системе настроен `pam_fprintd`.
- Отказ PAM → сессия остаётся заблокированной; крэш → systemd перезапускает и **сразу блокирует снова**.
- `logind SetLockedHint(true)` для остальных приложений.

## 6. Трей

Полный SNI: `IconPixmap` (ARGB32, network byte order), `IconName` (поиск в icon-theme), `ToolTip`, `Menu` (`dbusmenu`, рекурсия, ленивая подгрузка), `Activate`/`SecondaryActivate`/`Scroll`, сигналы изменения свойств.
`XEmbed` (Wine, старый Electron) — **не поддерживаем**; документируем.

## 7. Клипборд (шифрованная история)

- Ключ: Secret Service (`shell/clipboard-key`), AES-256-GCM, шифруется весь блоб истории (текст + PNG).
- Keyring недоступен → история **отключается** с предупреждением (не пишем plaintext «на всякий случай»).
- Смена/потеря ключа → старая история нечитаема: предлагаем очистить, а не падаем.
- Политика: игнор `x-kde-passwordManagerHint`, исключения по приложению, TTL, лимит размера/количества, пины.

## 8. Оценка трудозатрат

| Блок | Дней |
|---|---|
| Нотификации + OSD + история | 5–7 |
| Трей (SNI + dbusmenu) | 15–20 |
| Аудио + микшер | 5 |
| Сеть (NM + agent + VPN) | 10–12 |
| Bluetooth (агент, батареи) | 8–10 |
| Лок + PAM + ESL | 8–10 |
| Idle/DPMS/политика | 3 |
| Клипборд (с шифрованием) | 5 |
| Яркость | 3 |
| Обои + ночной свет | 4 |
| Скриншот | 5 |
| Системный монитор | 3 |
| Погода/календарь | 3 |
