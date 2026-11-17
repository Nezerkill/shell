# shell

Нативный Wayland-шелл на Rust для **Hyprland** и **Niri** (оба — с первого дня).
Заменяет связку waybar + mako/swaync + fuzzel + hyprlock + hypridle + swww + polkit-агент
одним набором процессов.

## Документы

Архитектура согласована поэтапно и живёт в [`docs/architecture/`](docs/architecture/):

| # | Документ | Тема |
|---|---|---|
| 1 | [0001-foundations.md](docs/architecture/0001-foundations.md) | цели, инварианты, процессы, стек, модель плагинов, IPC |
| 2 | [0002-config-model.md](docs/architecture/0002-config-model.md) | Nix ↔ GUI ↔ live-apply, проекция в Nix |
| 3 | [0003-compositor.md](docs/architecture/0003-compositor.md) | абстракция Hyprland/Niri, capabilities, хоткеи |
| 4 | [0004-services.md](docs/architecture/0004-services.md) | D-Bus, нотификации, трей, лок, клипборд |
| 5 | [0005-ui.md](docs/architecture/0005-ui.md) | поверхности, попапы, рендер, виджеты |
| 6 | [0006-theme-plugins-api.md](docs/architecture/0006-theme-plugins-api.md) | тема, плагины, публичный D-Bus API |
| 7 | [0007-ai-packaging-roadmap.md](docs/architecture/0007-ai-packaging-roadmap.md) | AI, Nix-упаковка, roadmap |

## Состояние

Скелет workspace + спайки S0. Код в крейтах — заглушки до спринтов S1+.

```bash
nix develop          # тулчейн (rustc/cargo из nixpkgs)
cargo check --workspace
bash scripts/check-invariants.sh
```

## Структура

```
crates/   shell-proto, shell-config, shell-compositor, shell-services, shell-theme,
          shell-plugin-api, shell-plugin-host, shell-ai, shell-ipc, shell-tokens, shell-ui
bin/      shelld (демон), shell-ui (рендер), shell-lock (ESL+PAM), shellctl (CLI)
modules/  nixos/, home-manager/
spikes/   S0: R1–R7 (см. spikes/README.md)
```
