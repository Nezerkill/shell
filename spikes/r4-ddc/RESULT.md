# R4 — яркость внешних мониторов (DDC/CI, ddcutil/i2c)

Хост: NixOS 26.11 (nixpkgs `31ecbaca`), ядро 7.3.0, один GPU AMD Navi 10 (RX 5600 XT), `HDMI-A-1` 1920×1080. Воспроизводят `./run.sh`, `./selftest-stub.sh`.

## Вердикт

1. **Карта «выход → шина DDC» собирается из sysfs без root** — работает, таблица ниже.
2. **ddcutil 2.2.7 без `/dev/i2c-*` заблокирован полностью**: `No /dev/i2c devices exist.` / `ddcutil requires module i2c-dev.`, exit **1**, за **15–20 мс**, до обращения к шине; то же у `--bus 6 getvcp 10`, `--terse`, `--skip-ddc-checks`. **Время отклика DDC не измерено.**
3. **Sysfs-путь инструмента рабочий**: `ddcutil environment` (exit 0) обходит sysfs, находит все 10 шин, карту `0000:05:00.0`, `sysfs_i2c_devices_exist: true`. Блокирует preflight `/dev`.
4. **Флаги политики таймаутов есть**: `--brief`, `-t/--terse`, `--sleep-multiplier`, `--maxtries`, `-e/--edid`, `--display`, `--bus`, `--noverify`, `--disable-dynamic-sleep`. `--maxtries` принимает **ровно три** значения: `--maxtries 1` → `Option --maxtries requires 3 values`, exit 1; нужно `1,1,1`.
5. **Идентификация без i2c**: EDID из sysfs читается без root (XMI/10162 «Mi Monitor»); `/sys/class/backlight` пуст — kernel-пути яркости нет.

## Как проверял

`./run.sh` (перечисление `/sys/class/drm/card*-*`, `--version/--help/detect`, `--bus 6 getvcp 10`, замеры `date +%s%N`), `./selftest-stub.sh`, `ddcutil environment`, чтение `nixos/modules/hardware/i2c.nix` из nixpkgs `31ecbaca`. Без sudo/modprobe, без записи в `/sys`/`/dev`, без изменения состояния дисплея.

## Доказательства

| Выход | status | enabled | ddc → | № | адаптер |
|---|---|---|---|---|---|
| card1-DP-1 | disconnected | disabled | i2c-3 | 3 | AMDGPU DM i2c hw bus 0 |
| card1-DP-2 | disconnected | disabled | i2c-4 | 4 | AMDGPU DM i2c hw bus 1 |
| card1-DP-3 | disconnected | disabled | i2c-5 | 5 | AMDGPU DM i2c hw bus 2 |
| card1-HDMI-A-1 | **connected** | enabled | i2c-6 | 6 | AMDGPU DM i2c hw bus 3 |

Не-DDC шины: `i2c-0` SMBus I801, `i2c-1/2` SMU, `i2c-7/8/9` — «DM **aux** hw bus 0…2» под DP.

```
PASS  connected output: card1-HDMI-A-1 -> i2c-6 (AMDGPU DM i2c hw bus 3)
PASS  documented failure mode: rc=1, "requires module i2c-dev" before any sysfs/bus probing
PASS  no-access failure returned in 15 ms (no hang, no DDC timeout)
SKIP  getvcp 0x10 round-trip NOT measured: no readable/writable /dev/i2c-*
$ ddcutil --version -> 2.2.7 (запуск процесса 6 мс)
$ --maxtries 1 --bus 6 getvcp 10 -> "Option --maxtries requires 3 values", exit 1
```

## Вывод

- **Номер шины — не константа**: `i2c-3…6` выдаёт i2c-core по порядку probe (`i2c_i801` → 0, SMU → 1/2, DM hw bus 0…3 → 3…6); стабилен симлинк `/sys/class/drm/<connector>/ddc`, а не цифра. Шелл читает симлинк на hotplug и передаёт `--bus`; «6» не хардкодить. DP-монитор сам нумерацию не сдвигает (у DP-1 ddc уже `i2c-3`), сдвиг даёт порядок probe — **unverified**.
- **`--bus`, а не `--display`**: `--display` требует полного detect (скан всех 10 шин с DDC-паузами). Структурная оценка, чисел нет.
- **Бюджет «монитор отсутствует»**: измерен только отказ preflight (15–20 мс). MCCS-паузы и ретраи при молчащем мониторе — **unverified**. Отсюда: вызов не в UI-потоке, жёсткий кап (2 с — **unverified**), ретрай `--maxtries 1,1,1 --sleep-multiplier 0.5`.
- **Кэш**: ключ — коннектор + EDID/серийник (EDID доступен без root и до DDC); значение показывается мгновенно, обновление асинхронно.
- **Запись дороже чтения**: `setvcp` может включать проверочное чтение, документация противоречива (`--help`: `--verify` default; man: «`--noverify` is the default»). Для слайдера — явный `--noverify`, debounce, `--verify` только на финале; flock ddcutil сериализует вызовы.

## Что не проверено

- Время `getvcp 0x10` и поведение при отсутствии/выключенном мониторе: нет прав на i2c. `run.sh` измерит 5 раз (default и `1,1,1`/`0.5`), напечатает min/median/max, rc и латентность контрольной `i2c-3`; ветка проверена только на заглушке.
- Поддержка DDC/CI и VCP 0x10 монитором; ответ при выключенном питании.
- Сдвиг нумерации при реальном hotplug (менять состояние дисплея запрещено).
- uaccess/группа `i2c` после `hardware.i2c.enable = true` — не применял.
- Безопасность `--sleep-multiplier 0.5` на железе.
