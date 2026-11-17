#!/usr/bin/env bash
# Self-test for run.sh's *privileged* branch, without any i2c access.
# It fakes the two things the branch needs — /dev/i2c-* nodes in a temp dir and
# a ddcutil that answers like a real one — and checks that run.sh then produces
# measurements instead of SKIPs.  This proves the measurement code path works;
# it proves NOTHING about real DDC latency.
#
#   ./selftest-stub.sh        # exit 0 = run.sh's privileged branch behaves
set -uo pipefail
here=$(dirname "$(readlink -f "${BASH_SOURCE[0]}")")

TMP=$(mktemp -d) || exit 1
trap 'rm -rf "$TMP"' EXIT INT TERM

# fake buses: 6 = "the monitor", 3 = "bus with nothing behind it"
: >"$TMP/i2c-6"
: >"$TMP/i2c-3"
chmod 600 "$TMP/i2c-6" "$TMP/i2c-3"

cat >"$TMP/ddcutil-stub" <<'STUB'
#!/usr/bin/env bash
args=("$@")
for i in "${!args[@]}"; do
    if [ "${args[$i]}" = "--maxtries" ]; then
        case "${args[$((i + 1))]:-}" in *,*) ;; *) echo "   Option --maxtries requires 3 values"; exit 1 ;; esac
    fi
done
case " ${args[*]} " in
    *" --version "*) echo "ddcutil 2.2.7 (stub)"; exit 0 ;;
    *" --help "*)
        printf '%s\n' \
            '  -t, --terse                       Show brief detail' \
            '  --brief                           Show brief detail' \
            '  -d, --display=number              Display number' \
            '  -b, --bus=busnum                  I2C bus number' \
            '  -e, --edid=256 char hex string    Monitor EDID' \
            '  --sleep-multiplier=number         Multiplication factor for DDC sleeps' \
            '  --maxtries=comma separated list   Max try adjustment' \
            '  --noverify                        Do not verify the value by setvcp' \
            '  --disable-dynamic-sleep           Disable dynamic sleep algorithm'
        exit 0 ;;
    *" detect "*) echo "Display 1"; echo "   I2C bus: i2c-6"; exit 0 ;;
esac
bus=""
for i in "${!args[@]}"; do
    [ "${args[$i]}" = "--bus" ] && bus="${args[$((i + 1))]}"
done
[ -n "$bus" ] || bus=6   # --display 1 path
sleep 0.02
if [ "$bus" = 3 ]; then echo "DDC communication failed. (stub)"; exit 1; fi
echo "VCP code 0x10 (Brightness): current value = 42, max value = 100"
STUB
chmod +x "$TMP/ddcutil-stub"

out=$(R4_I2C_DIR="$TMP" R4_DDCUTIL="$TMP/ddcutil-stub" "$here/run.sh" 2>&1)
rc=$?
echo "$out" | sed -n '/\[3\]/,$p'

fail=0
chk() { if grep -q -- "$1" <<<"$out"; then echo "  PASS  $2"; else echo "  FAIL  $2"; fail=1; fi; }
chk 'is RW for' "privileged branch detected RW i2c node"
chk 'min=[0-9]* median=[0-9]* max=[0-9]*' "5-sample stats printed for both variants"
chk 'brightness read 5/5 rc=0' "5/5 reads succeeded"
chk 'current brightness parsed: 42/100' "VCP 0x10 value parsed"
chk 'no-display bus fails with non-zero rc' "absent-monitor bus reported a failure"
chk 'no-display failure bounded' "absent-monitor latency bounded"
chk 'detect (scan of every bus)' "detect latency measured"
chk 'forces a detection scan first' "--display latency measured"
[ "$rc" -eq 0 ] && echo "  PASS  run.sh exit 0" || { echo "  FAIL  run.sh exit $rc"; fail=1; }
[ "$fail" -eq 0 ] && echo "STUB SELFTEST: ok (stub numbers, not real hardware)" || echo "STUB SELFTEST: FAILED"
exit "$fail"
