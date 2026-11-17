#!/usr/bin/env bash
# R4 spike — brightness of external monitors over DDC/CI (ddcutil / i2c).
# Question: response time of a brightness read, and behaviour when no monitor
# answers.  This script never uses sudo, never writes to /sys or /dev, and never
# changes display state (read-only VCP 0x10 queries only).
#
#   ./spikes/r4-ddc/run.sh          # works inside and outside `nix develop`
#
# Exit 0 whenever the spike *skips* the privileged half (no /dev/i2c-*), so the
# sysfs assertions can still gate CI.
set -uo pipefail
cd "$(dirname "$(readlink -f "${BASH_SOURCE[0]}")")" || exit 1

# --- re-exec inside the project devShell (same pattern as r3/r7) -------------
if [ -z "${IN_NIX_SHELL:-}" ] && [ -z "${R4_IN_SHELL:-}" ] && command -v nix >/dev/null 2>&1; then
    echo "[run.sh] not in the devShell, re-exec via nix develop /home/nezerkill/shell"
    R4_IN_SHELL=1 exec nix develop /home/nezerkill/shell --no-warn-dirty -c bash "$(readlink -f "${BASH_SOURCE[0]}")" "$@"
fi

FAIL=0
pass() { printf '  PASS  %s\n' "$1"; }
fail() { printf '  FAIL  %s\n' "$1"; FAIL=1; }
skip() { printf '  SKIP  %s\n' "$1"; }
note() { printf '  note  %s\n' "$1"; }

TMP=$(mktemp -d) || exit 1
trap 'rm -rf "$TMP"' EXIT INT TERM

median() { # stdin: one integer per line
    sort -n | awk '{a[NR]=$1}
        END { if (NR==0) { print "n/a"; exit }
              if (NR%2) print a[(NR+1)/2]; else print int((a[NR/2]+a[NR/2+1])/2) }'
}

# timed FILE CMD... -> T_MS, T_RC, output in FILE
timed() {
    local f=$1 s e
    shift
    s=$(date +%s%N)
    "$@" >"$f" 2>&1
    T_RC=$?
    e=$(date +%s%N)
    T_MS=$(( (e - s) / 1000000 ))
}

# --- ddcutil binary: resolve the store path once, so timings exclude nix -----
# R4_DDCUTIL / R4_I2C_DIR exist only to exercise the privileged branch against a
# stub (see selftest-stub.sh); with no env set everything is the real thing.
DDCUTIL="${R4_DDCUTIL:-}"
if [ -z "$DDCUTIL" ]; then
    if out=$(nix build --no-link --print-out-paths nixpkgs#ddcutil 2>/dev/null); then
        DDCUTIL="$out/bin/ddcutil"
    fi
    if [ ! -x "$DDCUTIL" ] && command -v ddcutil >/dev/null 2>&1; then
        DDCUTIL=$(command -v ddcutil)
    fi
fi
I2C_DIR="${R4_I2C_DIR:-/dev}"
node_path() { printf '%s/i2c-%s' "$I2C_DIR" "$1"; }

echo "== environment =="
printf '  kernel  %s\n' "$(uname -r)"
printf '  user    %s (%s)\n' "$(id -un)" "$(id -u)"
printf '  ddcutil %s\n' "${DDCUTIL:-<not available>}"
if [ -n "$DDCUTIL" ]; then
    "$DDCUTIL" --version 2>/dev/null | head -1 | sed 's/^/          /'
    s=$(date +%s%N); "$DDCUTIL" --version >/dev/null 2>&1; e=$(date +%s%N)
    printf '  ddcutil process startup (--version): %d ms\n' $(( (e - s) / 1000000 ))
fi

# =============================================================================
echo
echo "== [1] sysfs enumeration: output -> DDC bus (no root needed) =="
CONNECTORS=()
for d in /sys/class/drm/card*-*/; do
    [ -e "$d/status" ] || continue
    CONNECTORS+=("$d")
done

printf '  %-14s %-13s %-9s %-8s %s\n' OUTPUT STATUS ENABLED BUS "I2C ADAPTER NAME"
PRIMARY="" PRIMARY_BUS="" ABSENT="" ABSENT_BUS=""
for d in "${CONNECTORS[@]}"; do
    name=$(basename "$d")
    st=$(cat "$d/status" 2>/dev/null || echo '?')
    en=$(cat "$d/enabled" 2>/dev/null || echo '?')
    bus=""
    if [ -L "$d/ddc" ] || [ -e "$d/ddc" ]; then
        bus=$(basename "$(readlink -f "$d/ddc")" 2>/dev/null | sed 's/^i2c-//')
    fi
    adap="-"
    [ -n "$bus" ] && [ -r "/sys/bus/i2c/devices/i2c-$bus/name" ] \
        && adap=$(cat "/sys/bus/i2c/devices/i2c-$bus/name")
    printf '  %-14s %-13s %-9s %-8s %s\n' "$name" "$st" "$en" "${bus:-none}" "$adap"
    if [ "$st" = connected ] && [ -n "$bus" ] && [ -z "$PRIMARY" ]; then
        PRIMARY="$name"; PRIMARY_BUS="$bus"
    fi
    if [ "$st" = disconnected ] && [ -n "$bus" ] && [ "${bus}" != "${PRIMARY_BUS:-}" ] && [ -z "$ABSENT" ]; then
        ABSENT="$name"; ABSENT_BUS="$bus"
    fi
done

[ "${#CONNECTORS[@]}" -gt 0 ] && pass "${#CONNECTORS[@]} card*-* outputs found in /sys/class/drm" \
    || fail "no /sys/class/drm/card*-* outputs at all"

no_ddc=0
for d in "${CONNECTORS[@]}"; do
    [ -L "$d/ddc" ] || [ -e "$d/ddc" ] || no_ddc=$((no_ddc + 1))
done
[ "$no_ddc" -eq 0 ] && pass "every output exposes a ddc symlink" \
    || fail "$no_ddc output(s) without a ddc symlink"

if [ -n "$PRIMARY" ]; then
    pass "connected output: $PRIMARY -> i2c-$PRIMARY_BUS ($(cat "/sys/bus/i2c/devices/i2c-$PRIMARY_BUS/name" 2>/dev/null))"
    prim_name=$(cat "/sys/bus/i2c/devices/i2c-$PRIMARY_BUS/name" 2>/dev/null || echo)
    case "$prim_name" in
        *"DM i2c hw bus"*) pass "primary bus is a DDC-capable AMDGPU display bus (not SMU/SMBus)" ;;
        *) fail "primary bus '$prim_name' is not an AMDGPU DM i2c hw bus" ;;
    esac
else
    fail "no connected output with an i2c ddc link"
fi
if [ -n "$ABSENT_BUS" ]; then
    note "control bus for the 'no display' case: i2c-$ABSENT_BUS ($ABSENT), status disconnected"
else
    note "no disconnected output with a bus to use as the 'no display' control"
fi

# EDID is world-readable in sysfs: the monitor identity is available without any
# i2c access, which is what a brightness cache can be keyed on.
if [ -n "$PRIMARY" ] && [ -r "/sys/class/drm/$PRIMARY/edid" ]; then
    edid="/sys/class/drm/$PRIMARY/edid"
    hdr=$(od -An -tx1 -N 8 "$edid" 2>/dev/null | tr -d ' \n')
    if [ "$hdr" = "00ffffffffffff00" ]; then
        pass "EDID readable without root (header $hdr) => monitor identity is available pre-DDC"
    else
        fail "EDID header unexpected: ${hdr:-<empty>}"
    fi
else
    note "no readable EDID for ${PRIMARY:-<no connected output>}"
fi
note "no /sys/class/backlight entries: $(ls /sys/class/backlight/ 2>/dev/null | wc -l) device(s) -> no in-kernel brightness path for this panel"

# =============================================================================
echo
echo "== [2] ddcutil options needed by the design =="
if [ -z "$DDCUTIL" ] || [ ! -x "$DDCUTIL" ]; then
    skip "ddcutil unavailable (nix build nixpkgs#ddcutil failed and none in PATH)"
else
    "$DDCUTIL" --help >"$TMP/help.txt" 2>&1
    flags=("--brief" "-t, --terse" "--sleep-multiplier" "--maxtries" "-e, --edid=" "--display" "--bus" "--noverify" "--disable-dynamic-sleep")
    for f in "${flags[@]}"; do
        if grep -q -- "$f" "$TMP/help.txt"; then pass "ddcutil supports $f"; else fail "ddcutil lacks $f"; fi
    done
    # --maxtries takes exactly three values; a single number is rejected outright.
    timed "$TMP/mt.txt" "$DDCUTIL" --maxtries 1 --bus 1 getvcp 10
    if grep -q 'requires 3 values' "$TMP/mt.txt"; then
        pass "--maxtries 1 is rejected: \"$(head -1 "$TMP/mt.txt" | sed 's/^ *//')\" => must pass 1,1,1"
    else
        note "--maxtries 1 was not rejected on this build: $(head -1 "$TMP/mt.txt")"
    fi
fi

# =============================================================================
echo
echo "== [3] i2c access =="
nodes=()
for n in "$I2C_DIR"/i2c-*; do [ -e "$n" ] && nodes+=("$n"); done
ACCESS=0
pnode=$(node_path "${PRIMARY_BUS:-none}")
if [ "${#nodes[@]}" -eq 0 ]; then
    skip "no $I2C_DIR/i2c-* nodes exist (module i2c-dev not loaded) -> cannot run any DDC exchange"
    skip "module state: $(grep -c '^i2c_dev' /proc/modules 2>/dev/null || true) i2c_dev in /proc/modules; user groups: $(id -Gn)"
    skip "sudo is deliberately NOT attempted by this script; see UNBLOCK.md"
elif [ -z "$PRIMARY_BUS" ]; then
    skip "i2c nodes exist but no connected output/DDC bus was identified"
elif [ ! -e "$pnode" ]; then
    skip "i2c nodes exist but the primary bus $pnode is absent"
elif [ ! -r "$pnode" ] || [ ! -w "$pnode" ]; then
    skip "$pnode is not RW for $(id -un) ($(stat -c '%A %U:%G' "$pnode" 2>/dev/null))"
else
    ACCESS=1
    pass "$pnode is RW for $(id -un); ${#nodes[@]} i2c node(s) present"
fi

# =============================================================================
echo
echo "== [4] ddcutil detect (bus discovery) =="
if [ -n "$DDCUTIL" ] && [ -x "$DDCUTIL" ]; then
    timed "$TMP/detect.txt" "$DDCUTIL" --brief detect
    PREFLIGHT_MS=$T_MS
    printf '    | rc=%d in %d ms\n' "$T_RC" "$T_MS"
    sed 's/^/    | /' "$TMP/detect.txt" | head -8
    if [ "$ACCESS" -eq 1 ]; then
        [ "$T_RC" -eq 0 ] && pass "detect succeeded with i2c access (rc 0, ${T_MS} ms)" \
            || fail "detect failed with i2c access (rc $T_RC)"
    else
        if [ "$T_RC" -ne 0 ] && grep -q 'requires module i2c-dev' "$TMP/detect.txt"; then
            pass "documented failure mode: rc=$T_RC, \"requires module i2c-dev\" before any sysfs/bus probing"
        else
            fail "unexpected error without /dev/i2c-*: rc=$T_RC"
        fi
        # the failure is a /dev preflight, so it must be fast and must not hang
        [ "$T_MS" -lt 500 ] && pass "no-access failure returned in ${T_MS} ms (no hang, no DDC timeout)" \
            || fail "no-access failure took ${T_MS} ms"
        [ -z "$PRIMARY_BUS" ] || note "even '--bus $PRIMARY_BUS getvcp 10' is gated: $( ( "$DDCUTIL" --bus "$PRIMARY_BUS" getvcp 10 2>&1 || true ) | head -1)"
    fi
else
    skip "ddcutil unavailable: detect not tested"
fi

# =============================================================================
echo
echo "== [5] brightness read latency (only when i2c access exists) =="
if [ "$ACCESS" -ne 1 ]; then
    skip "getvcp 0x10 round-trip NOT measured: no readable/writable /dev/i2c-* (needs i2c-dev + uaccess/group i2c)"
    skip "no-monitor failure latency NOT measured: same reason (i2c bus cannot be opened at all)"
    skip "run ./run.sh again after UNBLOCK.md; it will fill in all numbers below and print them here"
    note "the only latency measured here is the ${PREFLIGHT_MS:-?} ms /dev preflight — that is NOT the DDC response time"
else
    # 5 reads with stock settings, then 5 with an aggressive retry/timeout budget
    for variant in default fast; do
        if [ "$variant" = default ]; then
            extra=()
        else
            extra=(--maxtries 1,1,1 --sleep-multiplier 0.5 --disable-dynamic-sleep)
        fi
        samples=(); ok=0; last_value=""
        echo "  -- $variant: ddcutil --brief --bus $PRIMARY_BUS ${extra[*]} getvcp 10"
        for i in 1 2 3 4 5; do
            timed "$TMP/v.txt" "$DDCUTIL" --brief --bus "$PRIMARY_BUS" "${extra[@]}" getvcp 10
            samples+=("$T_MS")
            [ "$T_RC" -eq 0 ] && ok=$((ok + 1))
            last_value=$(sed -n 's/.*current value = *\([0-9][0-9]*\).*/\1/p' "$TMP/v.txt" | head -1)
            printf '    | #%d rc=%d %5d ms  %s\n' "$i" "$T_RC" "$T_MS" "$(tr '\n' ' ' <"$TMP/v.txt" | cut -c1-90)"
        done
        mn=$(printf '%s\n' "${samples[@]}" | sort -n | head -1)
        mx=$(printf '%s\n' "${samples[@]}" | sort -n | tail -1)
        md=$(printf '%s\n' "${samples[@]}" | median)
        printf '    | %s: min=%s median=%s max=%s ms (n=%d)\n' "$variant" "$mn" "$md" "$mx" "${#samples[@]}"
        [ "$ok" -eq 5 ] && pass "$variant brightness read 5/5 rc=0" || fail "$variant brightness read only $ok/5 rc=0"
        case "$last_value" in
            ''|*[!0-9]*) fail "could not parse 'current value' from getvcp output" ;;
            *) [ "$last_value" -ge 0 ] && [ "$last_value" -le 100 ] \
                   && pass "$variant current brightness parsed: $last_value/100" \
                   || fail "$variant brightness out of range: $last_value" ;;
        esac
        eval "MED_$variant=$md; MAX_$variant=$mx"
    done
    [ -n "${MED_default:-}" ] && [ -n "${MED_fast:-}" ] \
        && note "budget: default median ${MED_default} ms, capped (maxtries 1,1,1 / sleep 0.5) median ${MED_fast} ms"

    # --- control: an i2c bus with no DDC display behind it -------------------
    if [ -n "$ABSENT_BUS" ]; then
        echo "  -- control: --bus $ABSENT_BUS ($ABSENT, disconnected), no DDC responder"
        for variant in default fast; do
            if [ "$variant" = default ]; then extra=(); else extra=(--maxtries 1,1,1 --sleep-multiplier 0.5 --disable-dynamic-sleep); fi
            timed "$TMP/a.txt" "$DDCUTIL" --brief --bus "$ABSENT_BUS" "${extra[@]}" getvcp 10
            printf '    | %s: rc=%d %5d ms  %s\n' "$variant" "$T_RC" "$T_MS" "$(tr '\n' ' ' <"$TMP/a.txt" | cut -c1-100)"
            [ "$T_RC" -ne 0 ] && pass "$variant no-display bus fails with non-zero rc ($T_RC)" \
                || fail "$variant no-display bus unexpectedly returned rc 0"
            [ "$T_MS" -lt 10000 ] && pass "$variant no-display failure bounded: ${T_MS} ms" \
                || fail "$variant no-display call took ${T_MS} ms (would stall a bar refresh)"
            [ "$T_MS" -le 1000 ] || note "$variant no-display call exceeds 1 s: the shell must run it off the UI thread with a hard cap"
        done
        # how much of the latency is ddcutil's own detection scan?
        timed "$TMP/d2.txt" "$DDCUTIL" --brief detect
        note "detect (scan of every bus) took ${T_MS} ms => --bus avoids this cost"
        timed "$TMP/d1.txt" "$DDCUTIL" --brief --display 1 getvcp 10
        note "--display 1 getvcp 10 took ${T_MS} ms (rc=$T_RC): --display forces a detection scan first"
    else
        skip "no disconnected output to use as the 'monitor absent' control"
    fi
fi

# =============================================================================
echo
if [ "$FAIL" -eq 0 ]; then
    echo "RESULT: assertions passed (skips above are environmental, not failures)"
else
    echo "RESULT: some assertions FAILED (see above)"
fi
exit "$FAIL"
