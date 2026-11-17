# R1 — `ext-session-lock-v1` из Rust, без готовых обёрток

Живая сессия (Hyprland + DMS, один HDMI) не трогалась: все локи — внутри **headless sway 1.12**
(wlroots 0.20.2), `WLR_BACKENDS=headless WLR_HEADLESS_OUTPUTS=2 WLR_RENDERER=pixman`, свой
`XDG_RUNTIME_DIR`, два виртуальных выхода.

Bindings генерируются на этапе компиляции из завендоренного `protocols/*.xml` макросами
`wayland_scanner::generate_interfaces!` / `generate_client_code!` (крейт `wayland-protocols` не
используется). Кадры — `wl_shm` (memfd, XRGB8888), рисуются вручную, без GPU и toolkit'ов. Бинарь
отказывается работать без `R1_NESTED=1`, поэтому залочить живую сессию им нельзя случайно.

## Вердикт

**Идём.** `./spikes/r1-esl/run.sh` → `RESULT: all assertions passed`, exit 0, 75/75 ассертов, ≈2,5 мин.

1. **Лок/анлок** — `locked` один раз, по `lock_surface` на выход (2/2), буферы ровно 921600 px,
   кадры разные; после `unlock_and_destroy` + `wl_display.sync` — `released`, стол вернулся.
2. **Два монитора** — grim видит на HEADLESS-1 кадр с `#1b2b4a`, на HEADLESS-2 — с `#103a24`:
   поверхности легли на свои выходы, имя выхода подписано внутри кадра.
3. **SIGKILL при локе** — сессия **осталась залоченной**: sway пишет `session lock abandoned`, выходы
   залиты красным (921600 px `#ff0000`), 43 сэмпла вокруг краха — ни одного со столом; новый клиент
   получает `locked` (`Replacing abandoned lock`).
4. **Ресайз** — 1280x720 → 2560x1440 (3686400 px) → 1600x900 → `scale 2` (800x450) → обратно:
   12 перерисовок, buffer == configure до пикселя, ошибок протокола нет; 143 сэмпла grim (~22 Гц) за
   6.2 с — **ни одного кадра со столом**.
5. **Живой Hyprland** рекламирует `ext_session_lock_manager_v1` v1 (1 `wl_output`).

## Как проверял

`run.sh` поднимает отдельный headless sway на тест (trap убивает всё по PID и по пути к конфигу),
разлок — строка `unlock` в stdin, есть аварийный `--hold`. Блоки: `[0]` guard, `[1]` лок/анлок +
второй клиент при живом локе, `[2]` SIGKILL, `[3]` ресайз с непрерывной съёмкой grim, `[5]` read-only
`wayland-info`. «Виден стол» однозначен: фон `#00ff00`, кадры лока тёмные, «брошенный» фон `#ff0000`.

## Доказательства

```
$ ./spikes/r1-esl/run.sh
  PASS  advertised: ext_session_lock_manager_v1 v1      # живая сессия, ничего не биндили
  PASS  lock surfaces created = 2 / configure events = 2 / buffers 921600 px
  PASS  HEADLESS-1 while locked: green (desktop) pixels = 0; dominant #1b2b4a
  PASS  second lock refused with 'finished' (exit 5) while a live client holds the lock
  PASS  'released' after unlock_and_destroy + wl_display.sync
  PASS  after kill: sway's 'abandoned lock' backdrop = #ff0000 (921600 px)
  PASS  sway log: 'session lock abandoned' / 'Replacing abandoned lock'
  PASS  buffer at 2560x1440 has exactly 3686400 pixels; PNG dumped
RESULT: all assertions passed
```

Кадры: `frames/t1..t3/*.png` (`locked-2560x1440.png`, `locked-after-kill.png`).

## Вывод

Протокол реализуется целиком на `wayland-client` + `wl_shm`. Главная работа — обработка `configure`
(размер буфера обязан совпадать, иначе `dimensions_mismatch` убивает клиент) и выход после
`unlock_and_destroy`: уничтожать `wl_surface` после разлока нельзя — композитор уже снёс role-объект
(sway: `Protocol error 4 … surface was destroyed before its role object`), надо закрыть соединение.

## Что не проверено

- Настоящие два монитора и горячее отключение выхода при локе.
- Hyprland под локом: ставить лок на живой сессии запрещено.
- Ввод: в headless нет seat.

## Что записать в docs/0003-compositor.md

- **Версия:** только v1; запросы `lock`, `get_lock_surface`, `ack_configure`, `unlock_and_destroy`.
  События: `locked` | `finished` — одно на объект (после `unlock_and_destroy` объект уже
  уничтожен, события нет); `configure(serial,w,h)` на каждую lock surface при создании и при смене
  геометрии выхода.
- **Порядок:** sway присылает `locked` **до** появления lock surface, значит композитор сам закрывает
  выходы (у sway — чёрным); `locked` ≠ «кадр уже на экране».
- **Жизненный цикл на выход:** один `wl_surface` + `get_lock_surface` на `wl_output` (повтор —
  `duplicate_output`); `ack_configure` до коммита; буфер ровно w×h, null-буфер — ошибка протокола.
- **Ресайз:** новый `configure` при смене mode/scale; игнорировать нельзя — `dimensions_mismatch`
  убивает клиент. Старый буфер держится до коммита нового, поэтому кадр не раскрывается.
- **Крах:** `destroy` при активном локе — ошибка; исчезновение клиента лок не снимает (sway: сплошной
  фон + приём нового lock-клиента).
