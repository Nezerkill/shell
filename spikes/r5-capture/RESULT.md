# R5 — `ext-image-copy-capture-v1` + `ext-image-capture-source-v1`

Спайк: `spikes/r5-capture/{probe-crate,vendored}`, воспроизводит `./run.sh` (7 блоков, все assert'ы прошли, exit 0).
Хост: NixOS, живой Hyprland 0.56.2, devShell `/home/nezerkill/shell` — rustc/cargo 1.99.0, wayland-protocols 1.49, wayland-scanner 1.26.0.

## Вердикт

1. **Крейт подходит — берём его.** crates.io `wayland-protocols` **0.32.13** (latest stable) отдаёт оба протокола за фичами `client` + `staging`:
   `wayland_protocols::ext::image_copy_capture::v1::client` и `wayland_protocols::ext::image_capture_source::v1::client`. Vendored XML остаётся резервом и в сборку не идёт.
2. **В devshell wayland-protocols 1.49, оба XML — staging, не stable.** Пути: `…/share/wayland-protocols/staging/ext-image-copy-capture/ext-image-copy-capture-v1.xml` и `…/staging/ext-image-capture-source/ext-image-capture-source-v1.xml`.
   `<protocol>` номера версии не несёт; версии стоят на интерфейсах — 4 + 3, все `version="1"`. `ext_image_capture_source_v1` помечен `frozen="true"`: правки только новым major'ом.
3. **Крейт не отстал от пакета:** XML внутри 0.32.13 байт-в-байт равны копиям из 1.49 (sha256 `41a44665…`, `4ef41d15…`).
4. **Живой композитор умеет:** Hyprland 0.56.2 отдаёт `ext_image_copy_capture_manager_v1` (name 65), `ext_output_image_capture_source_manager_v1` (63), `ext_foreign_toplevel_image_capture_source_manager_v1` (64) — все v1. Fallback на `wlr-screencopy` не нужен.
5. **Vendored-путь тоже рабочий:** 3 XML (третий — `ext-foreign-toplevel-list-v1.xml`, нужен из-за `ext_foreign_toplevel_handle_v1`), C генерит системный сканер, Rust-биндинги собираются и биндят те же глобалы.

## Как проверял

- **probe A (`probe-crate`)** — только крейт: перечисляет модульные пути и типы (`Request::CreateSession`, `Options::PaintCursors`, `FailureReason`, все 7 интерфейсов), затем биндит три менеджера на живом сокете, не отправляя ни одного запроса. Фича `staging` обязательна: `cargo build --no-default-features` → exit 101, `cannot find image_copy_capture in ext`.
- **probe B (`vendored`)** — XML лежат в `vendored/protocols/`; `build.rs` прогоняет системный `wayland-scanner client-header` + `private-code` по всем трём и assert'ит `*_interface` и таблицы `wl_message`; сами биндинги — макросы крейта `wayland-scanner` (тянет ещё `wayland-backend` и `bitflags`).
- Живое подключение обоих бинарей: `bind` менеджеров + `roundtrip`; ни одного `create_session`/`capture`, кадры не захватывались.
- `wayland-info` из `nix shell nixpkgs#wayland-utils`, 71 интерфейс.

## Доказательства

```
$ ./run.sh
[1] wayland-protocols 1.49, pkgdatadir .../share/wayland-protocols
    staging/ext-image-copy-capture/ext-image-copy-capture-v1.xml (4 iface, все v1)
    staging/ext-image-capture-source/ext-image-capture-source-v1.xml (3 iface, все v1, frozen="true")
[2] Cargo.lock: wayland-protocols 0.32.13, wayland-client 0.31.15, wayland-scanner 0.31.11
    crates.io latest stable = 0.32.13, features: client/server/staging/unstable
    XML крейта == XML 1.49: 41a446653f78 / 4ef41d15e4cd
[3] crate-route: OK; live bound manager/output-source/toplevel-src, object_id 3/4/5, version 1
[4] без staging: exit 101, "cannot find image_copy_capture in ext"
[5] wayland-scanner 1.26.0: icc .h 974 стр./37 070 B, .c 135 стр./5 515 B; ics .h 310/13 768, .c 84/3 232
[6] vendored-route: OK; 7 интерфейсов с name+version, те же 3 глобала
[7] Hyprland: 65 ext_image_copy_capture_manager_v1, 63 ext_output…, 64 ext_foreign_toplevel…
RESULT: all assertions passed
```

## Вывод

В `Cargo.toml` ядра/крейта захвата:

```toml
wayland-client = "0.31"
wayland-protocols = { version = "0.32", features = ["client", "staging"] }
```

Своих `default`-фич у крейта нет, поэтому `client` и `staging` перечисляем явно — забытый `staging` даёт не пустой модуль, а ошибку компиляции (проверено).

## Что записать в docs/0003-compositor.md

- Захват экрана — `ext-image-copy-capture-v1` + `ext-image-capture-source-v1` (staging, интерфейсы v1; в описании XML «testing phase»), через крейт `wayland-protocols 0.32` + `wayland-client 0.31`, фичи `["client","staging"]` обязательны.
- Версии держать синхронно: wayland-protocols ≥ 1.49 в devShell, крейт ≥ 0.32; staging меняется без обратной совместимости, после обновления сверять sha256 крейтовых XML с store-копией.
- Три глобала: manager, output-source, foreign-toplevel-source. `wlr-screencopy`/pipewire fallback в MVP не тащим.
- `WAYLAND_PROTOCOLS` devShell не экспортирует; `pkg-config --variable=pkgdatadir wayland-protocols` отдаёт `//nix/store/…` (двойной слэш из пустого `pc_sysrootdir`) — путь в build-скриптах нормализовать.
- Vendored-путь, если понадобится: 3 XML + `wayland-backend` и `bitflags` в зависимостях, C-код — только для FFI/чужих сборок.

## Что не проверено

- Ни одной реальной сессии/кадра: не проверены shm/dmabuf буферы, `buffer_constraints`, `stopped`, оверлейный путь `create_pointer_cursor_session`.
- Права доступа: Hyprland 0.56 отдаёт менеджер без запроса разрешений; как это изменится в следующих версиях — не проверялось.
- Протокол в staging: следующий wayland-protocols может его заменить или убрать `frozen`-гарантии.
- Отсутствие глобалов на другом композиторе (Niri) и fallback — не проверялись.
- Многопоточный захват в фоне — не проверялся.
