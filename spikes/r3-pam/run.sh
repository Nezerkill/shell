#!/usr/bin/env bash
# Reproduce spike R3 from scratch: needs the project dev shell (cargo, rustc,
# pkg-config, linux-pam). Everything else is in this directory.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/../.." && pwd)"
cd "$repo"
exec nix develop --no-warn-dirty -c bash -c 'cd spikes/r3-pam && cargo run --quiet'
