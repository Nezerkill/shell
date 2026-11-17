#!/usr/bin/env bash
# R2 spike — wgpu blur / rounded corners at 4K@144 (offscreen render, no window).
# One command reproduces every number quoted in RESULT.md:
#   ./run.sh
# Works both inside and outside `nix develop` (it re-execs itself in the shell).
#
# The criterion from spikes/README.md — "frame <= 6.9 ms at 144 Hz" — is checked
# twice: by the binary itself and again here on the printed number.
set -uo pipefail
cd "$(dirname "$(readlink -f "${BASH_SOURCE[0]}")")" || exit 1

if ! command -v cargo >/dev/null 2>&1; then
    echo "[run.sh] no cargo in PATH, re-exec inside the shell devShell"
    exec nix develop /home/nezerkill/shell --no-warn-dirty -c bash "$(readlink -f "${BASH_SOURCE[0]}")" "$@"
fi

export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-6}" # 16 cores, ~6 GB free: stay polite
BIN=target/release/r2-render
LOG=out/last-run.log
BUDGET_MS=6.90
FAIL=0

pass() { printf '  PASS  %s\n' "$1"; }
fail() { printf '  FAIL  %s\n' "$1"; FAIL=1; }
note() { printf '  note  %s\n' "$1"; }

mkdir -p out

# --- environment -------------------------------------------------------------
echo "== environment =="
printf '  rustc %s / cargo %s\n' "$(rustc --version | cut -d' ' -f2)" "$(cargo --version | cut -d' ' -f2)"
WGPU_VER=$(awk '/^name = "wgpu"$/{getline; print; exit}' Cargo.lock | sed 's/.*"\(.*\)"/\1/')
printf '  wgpu %s (from Cargo.lock)\n' "${WGPU_VER:-unknown}"
[ "${WGPU_VER:-}" = "30.0.2" ] && pass "wgpu 30.0.2 (the version the spike was written against)" \
    || note "wgpu ${WGPU_VER:-?} differs from 30.0.2; API/behaviour may differ"

# wgpu loads libvulkan.so.1 with dlopen(); on NixOS nothing puts it in the
# loader path of a plain cargo binary, so wgpu finds no adapter and the spike
# would silently "skip". Take the libdir from the devShell's vulkan-loader.
VK_LIBDIR=$(pkg-config --variable=libdir vulkan 2>/dev/null || true)
if [ -n "${VK_LIBDIR}" ] && [ -e "${VK_LIBDIR}/libvulkan.so.1" ]; then
    export LD_LIBRARY_PATH="${VK_LIBDIR}${LD_LIBRARY_PATH:+:${LD_LIBRARY_PATH}}"
    printf '  libvulkan from %s (added to LD_LIBRARY_PATH)\n' "${VK_LIBDIR}"
else
    note "no libvulkan.so.1 from pkg-config; relying on the system loader path"
fi

# --- build -------------------------------------------------------------------
echo
echo "== build (release, -j 6) =="
if ! cargo build -j 6 --release --manifest-path Cargo.toml 2>&1 | tail -3; then
    echo "build FAILED"
    exit 1
fi
[ -x "$BIN" ] || { echo "$BIN missing"; exit 1; }
printf '  %s = %s bytes\n' "$BIN" "$(stat -c %s "$BIN")"

# --- run ---------------------------------------------------------------------
echo
echo "== run: 20 warm-up + 200 measured frames at 3840x2160 =="
out=$("$BIN" 2>&1)
rc=$?
echo "$out" | tee "$LOG" | sed 's/^/  | /'

if [ "$rc" -eq 77 ]; then
    echo
    echo "  SKIP  no Vulkan device is reachable (no ICD, or libvulkan.so.1 not"
    echo "        loadable) — nothing was measured, nothing is claimed."
    exit 0
fi

# --- assertions on the measured numbers --------------------------------------
echo
echo "== assertions =="
[ "$rc" -eq 0 ] && pass "binary exited 0 (all in-process checks passed)" \
    || fail "binary exited $rc"

if grep -q '^  FAIL' "$LOG"; then
    fail "in-process checks failed: $(grep -c '^  FAIL' "$LOG") of them"
    grep '^  FAIL' "$LOG" | sed 's/^/  | /'
else
    pass "every in-process check (blur vs CPU, flatness, edge, corners) passed"
fi

criterion=$(grep '^CRITERION ' "$LOG" | tail -1)
p95_gpu=$(sed -n 's/.*p95_gpu_ms=\([0-9.]*\).*/\1/p' <<<"${criterion}")
p95_cpu=$(sed -n 's/.*p95_cpu_ms=\([0-9.]*\).*/\1/p' <<<"${criterion}")
if [ -z "${p95_gpu}" ] || [ -z "${p95_cpu}" ]; then
    fail "no CRITERION line in the output — cannot judge the budget"
else
    printf '  numbers  gpu p95 %s ms, cpu p95 %s ms, budget %s ms\n' "${p95_gpu}" "${p95_cpu}" "${BUDGET_MS}"
    # Goodput is judged on the GPU timestamps; the CPU number is the fallback
    # when the adapter lacks timestamp queries (then p95_gpu is nan).
    checked=${p95_gpu}
    case "${checked}" in
        nan|"") checked=${p95_cpu} ;;
    esac
    if awk -v v="${checked}" -v b="${BUDGET_MS}" 'BEGIN{exit !(v<=b)}'; then
        printf '  CRITERION (spike criterion): frame p95 %s ms <= %s ms  PASS\n' "${checked}" "${BUDGET_MS}"
    else
        printf '  CRITERION (spike criterion): frame p95 %s ms > %s ms  FAIL\n' "${checked}" "${BUDGET_MS}"
        FAIL=1
    fi
fi

# --- artifacts ---------------------------------------------------------------
echo
echo "== artifacts (must stay small and inspectable) =="
for f in out/frame-4k-1of8.png out/blur-edge.png out/corner-panel.png out/corner-aa-r16.png; do
    if [ ! -s "$f" ]; then
        fail "$f missing"
        continue
    fi
    sz=$(stat -c %s "$f")
    if [ "$sz" -lt 204800 ]; then
        pass "$(basename "$f") ${sz} B (< 200 KiB)"
    else
        fail "$(basename "$f") ${sz} B is too large"
    fi
done

# --- summary -----------------------------------------------------------------
echo
if [ "$FAIL" -eq 0 ]; then
    echo "RESULT: all assertions passed"
else
    echo "RESULT: some assertions FAILED (see above)"
fi
exit "$FAIL"
