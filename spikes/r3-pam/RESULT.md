# R3 — аутентификация через PAM из Rust, без внешнего helper'а

Хост: NixOS, nixpkgs `31ecbaca…`; сборка и запуск — в devShell
`/home/nezerkill/shell` (cargo 1.99.0, pkg-config, linux-pam).

Код — raw FFI: `pam-client`/`pam-sys` не использовались, зависимостей нет, флаги линковки даёт `pkg-config pam`.

## Вердикт

1. **Линковка + чтение `/etc/pam.d` — работает.** `pam_start("login")` → `PAM_SUCCESS`, `pam_end` → `PAM_SUCCESS`. Слинковано с системным `libpam.so.0` (1.7.2).
2. **Негативная аутентификация — работает.** Одна попытка, заведомо неверный пароль, пользователь `nezerkill` → **`PAM_AUTH_ERR (7)`**, а не `SERVICE_ERR (3)/SYSTEM_ERR (4)/CONV_ERR (19)`: стек реально доходит до `pam_unix` и оценивает пароль.
3. **Свой confdir — работает.** libpam 1.7.2 экспортирует `pam_start_confdir@@LIBPAM_1.4`. `confdir/permit` (`auth required pam_permit.so`) → `PAM_SUCCESS`; `confdir/deny` (`auth required pam_deny.so`) → `PAM_AUTH_ERR (7)`. `/etc` не трогали.
4. **NixOS-механизм — работает.** Опция `security.pam.services` существует; `builtins.attrNames` отдаёт 30 сервисов (`login`, `su`, `sudo`, `sddm`, `swaylock`, `i3lock`, …). `/etc/pam.d/shell` пока нет — его надо добавить в dotfiles.
5. **Маршрут версий — общий билд, но на диске есть второй.** Мы линкуемся на `/nix/store/18yk…-linux-pam-1.7.2` — ровно тот префикс, что указан в `/etc/pam.d/login` для `pam_unix.so`. При этом в store лежит второй linux-pam 1.7.2 (`fil0…`, другой sha256 .so): «чужой» билд появляется именно так.

## Доказательства

```
$ cd /home/nezerkill/shell && ./spikes/r3-pam/run.sh
libpam mapped: /nix/store/18yk…-linux-pam-1.7.2/lib/libpam.so.0.85.1
[1] pam_start("login")  -> start: PAM_SUCCESS (0) "Success"
[2] pam_authenticate("login", nezerkill, wrong pw)
    [conv] prompt(style=1): "Password: " -> <masked>
    -> pam_authenticate: PAM_AUTH_ERR (7) "Authentication failure"
[3a] pam_start_confdir(… confdir/permit) -> pam_authenticate: PAM_SUCCESS (0)
[3b] pam_start_confdir(… confdir/deny)   -> pam_authenticate: PAM_AUTH_ERR (7)
=== summary ===  4/4 PASS
$ ldd target/debug/r3-pam | grep pam
libpam.so.0 => /nix/store/18yk…-linux-pam-1.7.2/lib/libpam.so.0
$ nm -D --defined-only /nix/store/18yk…/lib/libpam.so.0 | grep pam_start_confdir
000000000000bf70 T pam_start_confdir@@LIBPAM_1.4
$ pkg-config --modversion pam
1.7.2
$ grep pam_unix.so /etc/pam.d/login
auth sufficient /nix/store/18yk…-linux-pam-1.7.2/lib/security/pam_unix.so likeauth nullok try_first_pass
$ (dotfiles) nix eval --json .#nixosConfigurations.nixos.config.security.pam.services \
      --apply builtins.attrNames
["chfn",…,"i3lock","kde","login","other","passwd",…,"sddm","su","sudo","swaylock",…]
$ faillock --user nezerkill      # пусто: одной попытки для лока не хватило
```

## Правки архитектуры

- **Линковать libpam из того же nixpkgs, что генерирует `/etc/pam.d`.** Оба flake-лока уже на `31ecbaca`, поэтому store-путь совпадает; нельзя менять rev только в одном из репозиториев и нельзя брать готовый helper из чужого кэша (кейс Noctalia).
- **Тесты lock screen — через `pam_start_confdir`** с `confdir/` в репозитории. Короткие имена модулей резолвятся в `<libpam>/lib/security` (там есть `pam_permit/deny/unix`), так что стек воспроизводим без root и без `/etc`.
- **`security.pam.services.shell`** в dotfiles вместо ручного `/etc/pam.d/shell`; держать в стеке `pam_unix.so try_first_pass`; если добавить `pam_faillock`, он считает неудачи по пользователю/TTY — нужен лимит и пауза между попытками.
- Один процесс — один libpam: если lock screen потянет ещё PAM-зависимости (kwallet, gnome-keyring), следить, чтобы все они пришли из того же билда.

## Не проверено

- Успешный анлок настоящим паролем: не знаем пароль и не вводили его (политика «одна попытка»); проверен только отказ.
- Реальный ABI-конфликт при смешивании rev: на хосте в стеке один билд, второй (`fil0…`) не задействован — отказ не воспроизводили.
- Модули, требующие `PAM_TTY`/`PAM_RHOST` (`pam_faillock`, `pam_systemd`) в приватном стеке не гонялись; в `login` faillock отсутствует.
- Поведение при запуске lock screen от root/через setuid-обёртку не проверено.
