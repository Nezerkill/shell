# R4 — как разблокировать измерение DDC

Спайк ничего не применяет: все команды ниже запускает пользователь сам.
Текущее состояние (проверено):

- `/dev/i2c-*` нет, `/sys/class/i2c-dev` нет;
- модуль `i2c_dev` не загружен и не вкомпилирован в ядро 7.3.0, но загружаемый модуль есть;
- группы `i2c` нет, udev-правила `70-i2c.rules` нет;
- ddcutil 2.2.7 отказывается работать без `/dev/i2c-*` — даже `--bus N`:
  `No /dev/i2c devices exist.` / `ddcutil requires module i2c-dev.` (exit 1).

## Шаг 1 — измерить прямо сейчас, без правки конфигов

Модуль + запуск ddcutil от root (root не зависит от прав на `/dev/i2c-*`):

```bash
sudo modprobe i2c-dev
ls -l /dev/i2c-*                     # должны появиться 10 узлов
sudo "$(nix build --no-link --print-out-paths nixpkgs#ddcutil)/bin/ddcutil" --bus 6 getvcp 10
```

После `modprobe` узлы обычно `root:root 0600`, поэтому `./run.sh` от пользователя
всё ещё скажет SKIP — измерению это не мешает, но для скрипта нужен шаг 2.

Проверить, что шина вообще отвечает (до ddcutil):

```bash
sudo "$(nix build --no-link --print-out-paths nixpkgs#i2c-tools)/bin/i2cdetect -y -r 6"
```

Если монитор не отвечает, включить DDC/CI в OSD монитора и повторить с
`--sleep-multiplier 2` (у части мониторов обмен медленный).

## Шаг 2 — постоянно, через NixOS (в `~/dotfiles`, применить самому)

Спайк этот файл не трогает. В `system/configuration.nix` (или отдельным модулем,
который туда импортирован):

```nix
{ pkgs, ... }:
{
  # boot.kernelModules = [ "i2c-dev" ] + users.groups.i2c + udev-правило 70-i2c.rules
  hardware.i2c.enable = true;

  # hardware.i2c.enable создаёт группу i2c, но не добавляет в неё пользователя
  users.users.nezerkill.extraGroups = [ "i2c" ];
}
```

Что именно генерирует `hardware.i2c` (проверено чтением
`nixos/modules/hardware/i2c.nix` из nixpkgs `31ecbaca`):

```udev
ACTION!="remove", KERNEL=="i2c-[0-9]*", TAG+="uaccess", GROUP="i2c", MODE="660"
```

То есть: `TAG+="uaccess"` даёт ACL активной сессии (Hyprland в seat) — этого
достаточно для CLI; `GROUP="i2c"` нужен службам вне seat (systemd user unit без
logind-сессии), поэтому членство в группе оставлено.

Эквивалент без опции, если понадобится вручную (не обязательно):

```nix
boot.kernelModules = [ "i2c-dev" ];
services.udev.extraRules = ''ACTION!="remove", KERNEL=="i2c-[0-9]*", TAG+="uaccess", GROUP="i2c", MODE="660"'';
users.groups.i2c = { };
users.users.nezerkill.extraGroups = [ "i2c" ];
```

Применение и проверка:

```bash
sudo nixos-rebuild switch --flake .#nixos
ls -l /dev/i2c-*                     # ожидаем crw-rw----+ root i2c
getent group i2c                     # nezerkill должен быть в списке (после нового логина)
cd /home/nezerkill/shell && ./spikes/r4-ddc/run.sh
```

`./run.sh` только читает VCP 0x10 (`getvcp`), ничего не пишет в монитор и не
меняет состояние дисплея. Если uaccess не сработал (например, запуск из ssh/tty),
выйти и войти в сессию заново.
