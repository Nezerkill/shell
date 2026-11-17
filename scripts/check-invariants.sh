#!/usr/bin/env bash
# Проверка архитектурных инвариантов из docs/architecture/0001-foundations.md.
set -euo pipefail
cd "$(dirname "$0")/.."
fail=0

# INV8: код, специфичный для композитора, живёт только в crates/shell-compositor/src/{hyprland,niri}
hits=$(grep -rn --include=*.rs -E 'hyprctl|niri msg|NIRI_SOCKET|HYPRLAND_INSTANCE_SIGNATURE' crates bin \
        | grep -v '^crates/shell-compositor/src/' || true)
if [ -n "$hits" ]; then
  echo "INV8 НАРУШЕН: обращение к композитору вне shell-compositor:"
  echo "$hits"
  fail=1
fi

# INV1: shell-ui не должен зависеть от сервисов/конфига напрямую
if command -v cargo >/dev/null 2>&1; then
  if cargo tree -p shell-ui --prefix none --no-dedupe 2>/dev/null | grep -qE '^(shell-services|shell-config) '; then
    echo "INV1 НАРУШЕН: shell-ui зависит от shell-services/shell-config"
    fail=1
  fi
fi

# INV7: shell-lock не зависит от плагинов и AI
if command -v cargo >/dev/null 2>&1; then
  if cargo tree -p shell-lock --prefix none --no-dedupe 2>/dev/null | grep -qE '^(shell-plugin-api|shell-plugin-host|shell-ai) '; then
    echo "INV7 НАРУШЕН: shell-lock зависит от плагинов/AI"
    fail=1
  fi
fi

[ "$fail" -eq 0 ] && echo "инварианты: ок"
exit "$fail"
