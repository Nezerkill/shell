#!/usr/bin/env bash
# R5 spike — ext-image-copy-capture-v1: crates.io crate or vendored XML?
# One command reproduces every number quoted in RESULT.md:
#   ./run.sh
# Works both inside and outside `nix develop` (it re-execs itself in the shell).
set -uo pipefail
cd "$(dirname "$(readlink -f "${BASH_SOURCE[0]}")")" || exit 1

if ! command -v cargo >/dev/null 2>&1; then
    echo "[run.sh] no cargo in PATH, re-exec inside the shell devShell"
    exec nix develop /home/nezerkill/shell --no-warn-dirty -c bash "$(readlink -f "${BASH_SOURCE[0]}")" "$@"
fi

FAIL=0
pass() { printf '  PASS  %s\n' "$1"; }
fail() { printf '  FAIL  %s\n' "$1"; FAIL=1; }
note() { printf '  note  %s\n' "$1"; }
hdr() { printf '\n== %s ==\n' "$1"; }

PROBE_BIN=probe-crate/target/debug/r5-probe-crate
VEND_BIN=vendored/target/debug/r5-vendored

# Is there a compositor to talk to? (Live checks degrade to notes without one.)
XDG_RUNTIME_DIR=${XDG_RUNTIME_DIR:-/run/user/$(id -u)}
live=0
if [ -n "${WAYLAND_DISPLAY:-}" ] && [ -S "$XDG_RUNTIME_DIR/$WAYLAND_DISPLAY" ]; then live=1; fi
export XDG_RUNTIME_DIR

printf '  rustc %s / cargo %s\n' "$(rustc --version | cut -d' ' -f2)" "$(cargo --version | cut -d' ' -f2)"

# --- [1] devshell: the wayland-protocols package itself ----------------------
hdr '[1] devshell: wayland-protocols package, XML paths, staging vs stable'
PKG_VER=$(pkg-config --modversion wayland-protocols 2>/dev/null)
PKGDATA_RAW=$(pkg-config --variable=pkgdatadir wayland-protocols 2>/dev/null)
PKGDATA=${PKGDATA_RAW#/} # pkg-config emits //nix/store/... (pc_sysrootdir is empty)
printf '  pkg-config wayland-protocols = %s\n  pkgdatadir = %s\n' "${PKG_VER:-<none>}" "$PKGDATA"

[ -n "$PKG_VER" ] && [ "$(printf '1.49\n%s\n' "$PKG_VER" | sort -V | head -1)" = "1.49" ] \
    && pass "wayland-protocols $PKG_VER (>= 1.49, the version this spike pins)" \
    || fail "wayland-protocols '${PKG_VER}' older than 1.49 (or pkg-config missing)"

ICC_XML=$PKGDATA/staging/ext-image-copy-capture/ext-image-copy-capture-v1.xml
ICS_XML=$PKGDATA/staging/ext-image-capture-source/ext-image-capture-source-v1.xml
for f in "$ICC_XML" "$ICS_XML"; do
    if [ -f "$f" ]; then pass "found $f"; else fail "missing $f"; fi
done
[ ! -e "$PKGDATA/stable/ext-image-copy-capture" ] && pass "not in stable/ — both protocols are staging" \
    || fail "ext-image-copy-capture appeared under stable/ (re-read this spike)"
if [ -z "${WAYLAND_PROTOCOLS:-}" ]; then
    note "WAYLAND_PROTOCOLS is not exported by the devShell; use pkg-config pkgdatadir"
fi

# Interface versions live on <interface>, not on <protocol>: print them all.
printf '  interfaces and versions:\n'
grep -ho 'interface name="[a-z_0-9]*" version="[0-9]*"' "$ICC_XML" "$ICS_XML" \
    | sed 's/.*name="\([a-z_0-9]*\)" version="\([0-9]*\)"/    \1 v\2/'
n_icc=$(grep -c '<interface ' "$ICC_XML")
n_ics=$(grep -c '<interface ' "$ICS_XML")
[ "$n_icc" -eq 4 ] && pass "ext-image-copy-capture-v1.xml: 4 interfaces" || fail "copy-capture has $n_icc interfaces, want 4"
[ "$n_ics" -eq 3 ] && pass "ext-image-capture-source-v1.xml: 3 interfaces" || fail "capture-source has $n_ics interfaces, want 3"
grep -q 'version="[0-9]"' <(grep -o '<protocol name="[a-z_0-9]*"[^>]*>' "$ICC_XML") \
    && fail "<protocol> carries a version attribute (unexpected)" \
    || pass "<protocol> has no version attribute; the .pc Version (1.49) is the package version"
grep -q 'frozen="true"' "$ICS_XML" \
    && pass "ext_image_capture_source_v1 is frozen=\"true\" in the XML (no further changes)" \
    || note "ext_image_capture_source_v1 is not marked frozen"

# --- [2] crates.io: what the crate offers ------------------------------------
hdr '[2] crates.io wayland-protocols: latest version and features'
LOCK_VP=$(grep -A1 'name = "wayland-protocols"' probe-crate/Cargo.lock | grep 'version' | head -1 | cut -d'"' -f2)
LOCK_WC=$(grep -A1 'name = "wayland-client"' probe-crate/Cargo.lock | grep 'version' | head -1 | cut -d'"' -f2)
LOCK_WS=$(grep -A1 'name = "wayland-scanner"' probe-crate/Cargo.lock | grep 'version' | head -1 | cut -d'"' -f2)
printf '  Cargo.lock: wayland-protocols %s, wayland-client %s, wayland-scanner %s\n' "$LOCK_VP" "$LOCK_WC" "$LOCK_WS"
case "$LOCK_VP" in
    0.32.*) pass "resolved wayland-protocols $LOCK_VP (the 0.32 series ships the protocol)" ;;
    *) fail "resolved wayland-protocols '$LOCK_VP', expected 0.32.x" ;;
esac

API=$(curl -sf -H 'User-Agent: r5-capture-spike (nezerkill)' https://crates.io/api/v1/crates/wayland-protocols 2>/dev/null)
if [ -n "$API" ]; then
    LATEST=$(grep -o '"max_stable_version":"[^"]*"' <<<"$API" | cut -d'"' -f4)
    pass "crates.io latest stable = $LATEST"
    FEATS=$(curl -sf -H 'User-Agent: r5-capture-spike (nezerkill)' \
        "https://crates.io/api/v1/crates/wayland-protocols/$LATEST" 2>/dev/null | grep -o '"features":{[^}]*}')
    printf '  features: %s\n' "$FEATS"
    grep -q '"staging"' <<<"$FEATS" && pass "crate has a 'staging' feature (the gate for this protocol)" \
        || fail "crate has no 'staging' feature"
    grep -q '"client"' <<<"$FEATS" && pass "crate has a 'client' feature" || fail "crate has no 'client' feature"
else
    note "crates.io API unreachable (offline); version taken from Cargo.lock only"
fi

# The crate bundles its own XML copy; it must be the same file the devShell has.
REG_VP=$(ls -d "$HOME"/.cargo/registry/src/*/wayland-protocols-"$LOCK_VP" 2>/dev/null | head -1)
if [ -n "$REG_VP" ]; then
    for pair in \
        "ext-image-copy-capture/ext-image-copy-capture-v1.xml:$ICC_XML" \
        "ext-image-capture-source/ext-image-capture-source-v1.xml:$ICS_XML"; do
        rel=${pair%%:*}; store=${pair#*:}
        a=$(sha256sum "$REG_VP/protocols/staging/$rel" | cut -d' ' -f1)
        b=$(sha256sum "$store" | cut -d' ' -f1)
        [ "$a" = "$b" ] && pass "crate-bundled ${rel##*/} is byte-identical to wayland-protocols 1.49 staging/ ($(cut -c1-12 <<<"$a"))" \
            || fail "crate-bundled ${rel##*/} differs from the devShell 1.49 XML"
    done
else
    note "crate sources not unpacked in ~/.cargo/registry, XML identity not checked"
fi

# --- [3] crate route: compile + live bind -----------------------------------
hdr '[3] crate route: probe-crate compiles, binds the live managers'
cargo build -q --manifest-path probe-crate/Cargo.toml >target/probe-build.log 2>&1
rc=$?
tail -5 target/probe-build.log | sed 's/^/    | /'
if [ "$rc" -eq 0 ]; then
    pass "probe-crate builds against wayland-protocols $LOCK_VP (features: client, staging)"
else
    fail "probe-crate does not build (exit $rc) — the crate lacks the protocol"
fi
out=$("$PROBE_BIN" 2>&1); rc=$?
echo "$out" | sed 's/^/    | /'
[ "$rc" -eq 0 ] && pass "probe-crate exits 0" || fail "probe-crate exits $rc"
grep -q '^crate-route: OK' <<<"$out" && pass "module paths resolved: wayland_protocols::ext::{image_copy_capture,image_capture_source}::v1::client" \
    || fail "crate-route did not complete"
grep -q 'type  ext_image_copy_capture_manager_v1 v1' <<<"$out" && pass "interface name+version come from the crate" || fail "no interface line"
if [ "$live" -eq 1 ]; then
    n=$(grep -c '^live  bound' <<<"$out")
    [ "$n" -eq 3 ] && pass "bound 3 live globals on the real compositor (manager, output source, toplevel source)" \
        || fail "bound $n live globals, want 3"
else
    note "no compositor socket: live bind skipped"
fi

# --- [4] negative control: without `staging` the modules do not exist --------
hdr '[4] negative control: --no-default-features (no `staging`) must NOT compile'
LOG=target/nostaging.log
cargo build -q --manifest-path probe-crate/Cargo.toml --no-default-features >"$LOG" 2>&1
rc=$?
if [ "$rc" -eq 0 ]; then
    fail "probe-crate built without the staging feature - the feature is not the gate we think"
else
    pass "build fails without staging (exit $rc): the feature is mandatory"
fi
grep -q 'cannot find `image_copy_capture` in `ext`' "$LOG" \
    && pass "compiler error is exactly \"cannot find image_copy_capture in ext\"" \
    || note "different compiler error text, see $LOG"
sed -n '1,3p' "$LOG" | sed 's/^/    | /' 

# --- [5] vendored XML: the system wayland-scanner ---------------------------
hdr '[5] vendored XML: wayland-scanner client-header + private-code'
mkdir -p target/codegen
gen_ok=1
for xml in ext-image-copy-capture-v1 ext-image-capture-source-v1 ext-foreign-toplevel-list-v1; do
    wayland-scanner client-header "vendored/protocols/$xml.xml" "target/codegen/$xml.h" || gen_ok=0
    wayland-scanner private-code  "vendored/protocols/$xml.xml" "target/codegen/$xml.c" || gen_ok=0
    printf '    %-32s .h %4s lines / %6s B   .c %3s lines / %5s B\n' "$xml" \
        "$(wc -l < "target/codegen/$xml.h")" "$(stat -c %s "target/codegen/$xml.h")" \
        "$(wc -l < "target/codegen/$xml.c")" "$(stat -c %s "target/codegen/$xml.c")"
done
[ "$gen_ok" -eq 1 ] && pass "wayland-scanner $(wayland-scanner --version 2>&1 | tr -s ' ' | cut -d' ' -f2) generated C for all three vendored XMLs" \
    || fail "wayland-scanner could not generate from the vendored XML"
grep -q 'ext_image_copy_capture_manager_v1_interface' target/codegen/ext-image-copy-capture-v1.c \
    && pass "private code defines ext_image_copy_capture_manager_v1_interface" || fail "interface symbol missing"
grep -q 'wl_message' target/codegen/ext-image-copy-capture-v1.c \
    && pass "private code carries wl_message tables (requests/events), not just names" || fail "no wl_message tables"
grep -q 'create_session' target/codegen/ext-image-copy-capture-v1.h \
    && pass "client header declares create_session/create_pointer_cursor_session" || fail "create_session missing"

# --- [6] vendored route: compile + generate + live bind ---------------------
hdr '[6] vendored route: probe B compiles from the XML and binds live'
cargo build -q --manifest-path vendored/Cargo.toml >target/vendored-build.log 2>&1
rc=$?
tail -5 target/vendored-build.log | sed 's/^/    | /'
if [ "$rc" -eq 0 ]; then
    pass "vendored crate builds; its build.rs asserted the C symbols at compile time"
else
    fail "vendored crate does not build (exit $rc)"
fi
REPORT=$(ls -t vendored/target/debug/build/r5-vendored-*/out/codegen-report.txt 2>/dev/null | head -1)
if [ -n "$REPORT" ]; then
    sed 's/^/    | /' "$REPORT"
    pass "build.rs codegen report found ($REPORT)"
else
    note "no codegen report (build script did not run in this configuration)"
fi
out=$("$VEND_BIN" 2>&1); rc=$?
echo "$out" | sed 's/^/    | /'
[ "$rc" -eq 0 ] && pass "vendored binary exits 0" || fail "vendored binary exits $rc"
grep -q '^vendored-route: OK' <<<"$out" && pass "bindings generated from vendored XML are usable" || fail "vendored route did not complete"
n=$(grep -c '^gen   ext_' <<<"$out")
[ "$n" -eq 7 ] && pass "7 generated interfaces carry name+version from the XML" || fail "$n generated interface lines, want 7"
if [ "$live" -eq 1 ]; then
    m=$(grep -c '^live  bound' <<<"$out")
    [ "$m" -eq 3 ] && pass "generated bindings bound 3 live globals (same objects as the crate route)" \
        || fail "generated bindings bound $m live globals, want 3"
else
    note "no compositor socket: live bind skipped"
fi

# --- [7] the live compositor's global list ----------------------------------
hdr '[7] live Hyprland: does it advertise the protocol?'
if WI=$(timeout 300 nix shell nixpkgs#wayland-utils -c wayland-info 2>/dev/null); then
    total=$(grep -c '^interface:' <<<"$WI")
    printf '  wayland-info: %s interfaces\n' "$total"
    for iface in ext_image_copy_capture_manager_v1 ext_output_image_capture_source_manager_v1 \
                 ext_foreign_toplevel_image_capture_source_manager_v1; do
        line=$(grep -o "'$iface', *version: *[0-9]*, *name: *[0-9]*" <<<"$WI")
        if [ -n "$line" ]; then pass "advertised: $line"; else fail "$iface not advertised"; fi
    done
    if [ "$live" -eq 1 ]; then
        grep -q 'ext_image_copy_capture_manager_v1' <<<"$WI" \
            && pass "the compositor this shell will run on speaks the protocol (no fallback needed)" \
            || fail "live compositor lacks the protocol"
    fi
else
    note "wayland-info unavailable (no network / no session); live global list not checked"
fi

# --- summary ----------------------------------------------------------------
echo
if [ "$FAIL" -eq 0 ]; then
    echo "RESULT: all assertions passed"
    echo "DECISION: wayland-protocols crate (0.32.x, features = [\"client\", \"staging\"]); vendored XML kept as the fallback."
else
    echo "RESULT: some assertions FAILED (see above)"
fi
exit "$FAIL"
