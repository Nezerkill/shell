#!/usr/bin/env bash
# R1 spike — ext-session-lock-v1 from Rust, without a session-lock wrapper crate.
#
#   ./run.sh
#
# Reproduces every number quoted in RESULT.md. Works both inside and outside
# `nix develop` (it re-execs itself in the dev shell; the compositor and the
# screenshot tool come from the nix store, so nothing has to be installed).
#
# SAFETY: this script NEVER touches the live session. Every lock happens inside
# a *headless* sway (WLR_BACKENDS=headless, own XDG_RUNTIME_DIR) whose only
# outputs are two virtual ones. The lock client refuses to run unless
# R1_NESTED=1, which only this script sets. The single live-session interaction
# is a read-only `wayland-info`.
#
# Colours are chosen so that "desktop content is visible" is unambiguous:
#   desktop background = #00ff00 (green, from sway's `output * bg`)
#   lock frames        = dark blue/green/purple/brown, never pure green
#   sway's own "abandoned lock" backdrop after a client crash = #ff0000 (red)
set -uo pipefail
cd "$(dirname "$(readlink -f "${BASH_SOURCE[0]}")")" || exit 1

if ! command -v cargo >/dev/null 2>&1; then
    echo "[run.sh] no cargo in PATH, re-exec inside the dev shell"
    exec nix develop /home/nezerkill/shell --no-warn-dirty -c bash "$(readlink -f "${BASH_SOURCE[0]}")" "$@"
fi

SPIKE=$(pwd)
BIN="$SPIKE/target/debug/r1-esl"
OUT="$SPIKE/frames"
FAIL=0

pass() { printf '  PASS  %s\n' "$1"; }
fail() { printf '  FAIL  %s\n' "$1"; FAIL=1; }
note() { printf '  note  %s\n' "$1"; }
eq() {
    if [ "$2" = "$3" ]; then pass "$1 = $2"; else fail "$1 = $2 (want $3)"; fi
}
ge() {
    if [ "${2:-0}" -ge "$3" ] 2>/dev/null; then pass "$1 = $2 (>= $3)"; else fail "$1 = $2 (want >= $3)"; fi
}
# json helpers for the client's one-event-per-line log / ppm-stats output
jnum() { grep -o "\"$2\":[0-9]*" <<<"$1" | head -1 | cut -d: -f2; }
jstr() { grep -o "\"$2\":\"[^\"]*\"" <<<"$1" | head -1 | cut -d'"' -f4; }

PIDS=()
RUNDIRS=()
cleanup() {
    for p in "${PIDS[@]:-}"; do kill -9 "$p" 2>/dev/null; done
    for p in "${PIDS[@]:-}"; do wait "$p" 2>/dev/null; done
    for d in "${RUNDIRS[@]:-}"; do
        pgrep -f "sway.*$d/sway.conf" 2>/dev/null | while read -r p; do kill -9 "$p" 2>/dev/null; done
    done
    rm -rf "${RUNDIRS[@]:-}" 2>/dev/null
}
trap cleanup EXIT INT TERM

# ---------------------------------------------------------------- tools -----
resolve() { nix build --no-link --print-out-paths "$1" 2>/dev/null | tail -1; }
SWAY_PKG=$(resolve nixpkgs#sway)
SWAY="$SWAY_PKG/bin/sway"
SWAYMSG="$SWAY_PKG/bin/swaymsg"
GRIM="$(resolve nixpkgs#grim)/bin/grim"
WAYLAND_INFO="$(resolve nixpkgs#wayland-utils)/bin/wayland-info"
for t in "$SWAY" "$SWAYMSG" "$GRIM" "$WAYLAND_INFO"; do
    [ -x "$t" ] || { echo "missing tool: $t"; exit 1; }
done
rm -rf "$OUT"; mkdir -p "$OUT"

# The live session, captured before the tests override the environment.
LIVE_RT="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}"
LIVE_DISPLAY="${WAYLAND_DISPLAY:-wayland-1}"

printf '== environment ==\n'
printf '  %s (%s)\n' "$("$SWAY" --version | head -1)" "$SWAY_PKG"
printf '  live session: XDG_RUNTIME_DIR=%s WAYLAND_DISPLAY=%s\n' "$LIVE_RT" "$LIVE_DISPLAY"

# ---------------------------------------------------------------- build -----
printf '\n== build ==\n'
cargo build --manifest-path Cargo.toml -j 4 --quiet 2>&1 | sed 's/^/    | /' || { echo "build FAILED"; exit 1; }
printf '  rustc %s / cargo %s\n' "$(rustc --version | cut -d' ' -f2)" "$(cargo --version | cut -d' ' -f2)"
printf '  %s = %s bytes; vendored XML: %s\n' "$BIN" "$(stat -c %s "$BIN")" "$(ls protocols/*.xml | xargs -n1 basename | tr '\n' ' ')"

# ------------------------------------------------- 0. safety guard ----------
printf '\n== [0] the binary refuses to lock outside the nested compositor ==\n'
out=$(env -u R1_NESTED XDG_RUNTIME_DIR="$LIVE_RT" WAYLAND_DISPLAY="$LIVE_DISPLAY" \
    timeout 10 "$BIN" --hold 1 2>&1); rc=$?
printf '    | %s\n' "$(head -1 <<<"$out")"
eq "exit code without R1_NESTED" "$rc" 2
grep -q 'refusing to run' <<<"$out" && pass "refused before connecting to the live session" \
    || fail "no refusal message"

# -------------------------------------------- 5. live session (read-only) ---
printf '\n== [5] live session: does Hyprland advertise ext_session_lock_manager_v1? ==\n'
live=$(timeout 25 env XDG_RUNTIME_DIR="$LIVE_RT" WAYLAND_DISPLAY="$LIVE_DISPLAY" \
    "$WAYLAND_INFO" 2>&1); rc=$?
eq "wayland-info exit code (read-only)" "$rc" 0
lockline=$(grep "ext_session_lock_manager_v1" <<<"$live" | head -1)
if [ -n "$lockline" ]; then
    pass "advertised: $(sed -E "s/^interface: '//; s/', *version: */ v/; s/, *name: .*//" <<<"$lockline")"
else
    fail "ext_session_lock_manager_v1 not advertised"
fi
printf '    | %s wl_output global(s) on the live session\n' "$(grep -c "interface: 'wl_output'" <<<"$live" || true)"
printf '    | no lock object was created on the live session, wayland-info only bound globals\n'

# -------------------------------------------------------- sway helpers ------
start_sway() { # start_sway <tag>
    local tag=$1
    RUNDIR=$(mktemp -d "${TMPDIR:-/tmp}/r1-esl-$tag.XXXXXX")
    chmod 700 "$RUNDIR"
    RUNDIRS+=("$RUNDIR")
    printf 'bar { mode invisible }\noutput * bg #00ff00 solid_color\n' > "$RUNDIR/sway.conf"
    (
        XDG_RUNTIME_DIR="$RUNDIR" \
        WLR_BACKENDS=headless WLR_HEADLESS_OUTPUTS=2 \
        WLR_LIBINPUT_NO_DEVICES=1 WLR_RENDERER=pixman \
        "$SWAY" -d -c "$RUNDIR/sway.conf" > "$RUNDIR/sway.log" 2>&1 &
        echo $! > "$RUNDIR/sway.pid"
    )
    SWAYPID=$(cat "$RUNDIR/sway.pid")
    PIDS+=("$SWAYPID")
    for _ in $(seq 1 150); do [ -S "$RUNDIR/wayland-1" ] && break; sleep 0.1; done
    [ -S "$RUNDIR/wayland-1" ] || { echo "sway did not start, see $RUNDIR/sway.log"; return 1; }
    export XDG_RUNTIME_DIR="$RUNDIR" WAYLAND_DISPLAY=wayland-1
    SWAYSOCK=$(ls "$RUNDIR"/sway-ipc* 2>/dev/null | head -1)
    export SWAYSOCK
    note "nested sway pid $SWAYPID, runtime $RUNDIR, outputs: $(outputs | tr '\n' ' ')"
}

stop_sway() {
    kill -9 "$SWAYPID" 2>/dev/null
    wait "$SWAYPID" 2>/dev/null
}

outputs() {
    timeout 15 "$SWAYMSG" -t get_outputs 2>/dev/null |
        grep -oE '"name": "[A-Za-z0-9-]+"' | cut -d'"' -f4 | sort
}

# shot <basename> [output]: prints the pixel statistics as JSON and leaves a
# small PNG in $EVID (PNG, not PPM: the lock frames are flat colours, so the
# file is a few dozen kB instead of megabytes).
shot() {
    local name=$1 o=${2:-HEADLESS-1}
    local ppm="$RUNDIR/shot-$name.ppm"
    timeout 25 "$GRIM" -t ppm -o "$o" "$ppm" 2>/dev/null || return 1
    local st
    st=$("$BIN" ppm-stats "$ppm") || return 1
    timeout 25 "$GRIM" -t png -o "$o" "$EVID/$name.png" 2>/dev/null || true
    rm -f "$ppm"
    printf '%s' "$st"
}

wait_ready() { # wait_ready <jsonl> [seconds]
    local log=$1 t=${2:-20}
    for _ in $(seq 1 $((t * 10))); do
        grep -q '"ev":"ready"' "$log" 2>/dev/null && return 0
        grep -qE '"ev":"(refused|fatal)"' "$log" 2>/dev/null && return 1
        sleep 0.1
    done
    return 1
}

wait_paints() { # wait_paints <jsonl> <min count> [seconds]
    local log=$1 n=$2 t=${3:-6}
    for _ in $(seq 1 $((t * 10))); do
        [ "$(grep -c '"ev":"configure"' "$log" 2>/dev/null)" -ge "$n" ] && return 0
        sleep 0.1
    done
    return 1
}

last_size() { # last configure size for one output: "WxH"
    grep '"ev":"configure"' "$1" 2>/dev/null | grep "\"output\":\"$2\"" | tail -1 |
        grep -o '"w":[0-9]*,"h":[0-9]*' | sed -E 's/"w":([0-9]+),"h":([0-9]+)/\1x\2/'
}

wait_last_size() { # wait_last_size <jsonl> <output> <WxH> [seconds]
    local log=$1 out=$2 wh=$3 t=${4:-8}
    for _ in $(seq 1 $((t * 10))); do
        [ "$(last_size "$log" "$out")" = "$wh" ] && return 0
        sleep 0.1
    done
    return 1
}

start_capture() { # start_capture <outdir> [output]
    local dir=$1 o=${2:-HEADLESS-1}
    CAPSTOP="$RUNDIR/stop-capture"
    rm -f "$CAPSTOP"
    : > "$RUNDIR/caps.tsv"
    ( i=0
      while [ ! -e "$CAPSTOP" ]; do
          i=$((i + 1))
          f=$(printf '%s/cap-%04d.ppm' "$dir" "$i")
          if timeout 10 "$GRIM" -t ppm -o "$o" "$f" 2>/dev/null; then
              printf '%s\t%s\n' "$(date +%s%3N)" "$f" >> "$RUNDIR/caps.tsv"
          fi
          sleep 0.01
      done ) &
    CAPLOOP=$!
    PIDS+=("$CAPLOOP")
}

stop_capture() {
    touch "$CAPSTOP"
    wait "$CAPLOOP" 2>/dev/null
    CAPLOOP=""
}

# "frames desktop_frames desktop_frames_after_the_first_lock_frame"
# The samples are ordered, so "desktop frames only as a prefix" means the
# compositor never went back to showing desktop content once it was locked.
frame_stats() {
    local dir=$1 n=0 d=0 tr=0 seen_lock=0 st f files
    files=$(ls "$dir"/cap-*.ppm 2>/dev/null | sort)
    [ -n "$files" ] || { printf '0 0 0\n'; return; }
    while IFS= read -r st; do
        n=$((n + 1))
        if [ "$(jnum "$st" green)" -gt 0 ]; then
            d=$((d + 1))
            [ "$seen_lock" -eq 1 ] && tr=$((tr + 1))
        else
            seen_lock=1
        fi
    done < <("$BIN" ppm-stats $files)
    printf '%s %s %s\n' "$n" "$d" "$tr"
}

# ================================================================ T1 =========
printf '\n== [1] lock + unlock, two outputs (nested sway, 2 headless outputs) ==\n'
start_sway t1 || exit 1
OUT1="$OUT/t1"; mkdir -p "$OUT1"

EVID="$OUT1"
pre=$(shot desktop-before)
eq "desktop before lock: green pixels" "$(jnum "$pre" green)" 921600
eq "desktop before lock: distinct colours" "$(jnum "$pre" distinct)" 1

# Sample the output across the lock acquisition.
mkdir -p "$RUNDIR/caps"
start_capture "$RUNDIR/caps" HEADLESS-1

mkfifo "$RUNDIR/stdin-lock"
R1_NESTED=1 "$BIN" --expect-outputs 2 --dump-dir "$OUT1" --unlock-on-stdin --hold 240 \
    --label "R1 T1" < "$RUNDIR/stdin-lock" > "$RUNDIR/t1.jsonl" 2> "$RUNDIR/t1.err" &
CPID=$!
PIDS+=("$CPID")
exec 7> "$RUNDIR/stdin-lock"

wait_ready "$RUNDIR/t1.jsonl" 20 && pass "client reached 'ready' (locked, every surface painted)" \
    || fail "client never reached 'ready': $(tail -2 "$RUNDIR/t1.jsonl")"
sleep 0.3
stop_capture
read -r caps1 green1 late1 <<<"$(frame_stats "$RUNDIR/caps")"
ge "frames sampled across lock acquisition" "$caps1" 1
note "$green1 of the $caps1 samples are the pre-lock desktop prefix; $late1 came after the first lock frame"
eq "desktop frames AFTER the lock first appeared" "${late1:-1}" 0

eq "locked events" "$(grep -c '"ev":"locked"' "$RUNDIR/t1.jsonl")" 1
eq "lock surfaces created" "$(grep -c '"ev":"lock_surface_created"' "$RUNDIR/t1.jsonl")" 2
eq "configure events" "$(grep -c '"ev":"configure_event"' "$RUNDIR/t1.jsonl")" 2
eq "distinct outputs with a lock surface" \
    "$(grep '"ev":"lock_surface_created"' "$RUNDIR/t1.jsonl" | grep -o '"output":"[^"]*"' | sort -u | wc -l)" 2
eq "ext_session_lock_manager_v1 bound" \
    "$(grep -c '"ev":"global","interface":"ext_session_lock_manager_v1"' "$RUNDIR/t1.jsonl")" 1

for i in 0 1; do
    line=$(grep '"ev":"configure"' "$RUNDIR/t1.jsonl" | sed -n "$((i + 1))p")
    name=$(jstr "$line" output); png=$(jstr "$line" png)
    eq "surface $i ($name) buffer size" "$(jnum "$line" w)x$(jnum "$line" h)" 1280x720
    eq "surface $i ($name) pixel count" "$(jnum "$line" pixels)" 921600
    [ -f "$png" ] && pass "surface $i PNG dumped: ${png#"$SPIKE"/}" || fail "no PNG for surface $i"
    awk -v v="$(grep -o '"known_frac":[0-9.]*' <<<"$line" | cut -d: -f2)" \
        'BEGIN{exit !(v>=0.9999)}' && pass "surface $i frame fully painted (known_frac >= 0.9999)" \
        || fail "surface $i frame not fully painted"
    note "surface $i: bg=$(jstr "$line" bg) hash=$(jstr "$line" hash) png=$(basename "$png")"
done
eq "two different frames (per-output label/colour)" \
    "$(grep '"ev":"configure"' "$RUNDIR/t1.jsonl" | grep -o '"hash":"[^"]*"' | sort -u | wc -l)" 2

for o in HEADLESS-1 HEADLESS-2; do
    s=$(shot "locked-$o" "$o")
    bg=$(grep '"ev":"configure"' "$RUNDIR/t1.jsonl" | grep "\"output\":\"$o\"" | grep -o '"bg":"#[0-9a-f]*"' | head -1 | cut -d'#' -f2 | tr -d '"')
    eq "$o while locked: green (desktop) pixels" "$(jnum "$s" green)" 0
    eq "$o while locked: dominant colour is that surface's bg" "$(jstr "$s" top)" "#$bg"
    ge "$o while locked: dominant-colour pixels" "$(jnum "$s" top_count)" 800000
    note "$o while locked: $(jnum "$s" distinct) colours, top_count=$(jnum "$s" top_count)/921600"
done
grep '"ev":"configure_event"' "$RUNDIR/t1.jsonl" | head -1 | grep -q '"locked_before":true' \
    && note "event order: locked arrived before the first configure (sway sends locked immediately)" \
    || note "event order: configure before locked"

printf -- '-- second lock attempt while locked --\n'
probe=$(R1_NESTED=1 timeout 20 "$BIN" --probe-lock --hold 5 2> "$RUNDIR/probe.err"); rc=$?
printf '    | %s\n' "$(grep -E '"ev":"(refused|protocol_error|probe_no_answer)"' <<<"$probe" | head -1)"
case "$rc" in
    5) pass "second lock refused with 'finished' (exit 5) while a live client holds the lock" ;;
    3) pass "second lock killed by a protocol error (exit 3) while a live client holds the lock" ;;
    *) fail "second lock attempt exit $rc (want 3 or 5)" ;;
esac

printf -- '-- unlock --\n'
if kill -0 "$CPID" 2>/dev/null; then
    echo unlock >&7
else
    fail "client died before the unlock request: $(tail -2 "$RUNDIR/t1.jsonl")"
fi
exec 7>&-
wait "$CPID"; rc=$?
eq "client exit code after unlock" "$rc" 0
grep -q '"ev":"released"' "$RUNDIR/t1.jsonl" && pass "'released' after unlock_and_destroy + wl_display.sync" \
    || fail "no 'released' event"
post=$(shot desktop-after-unlock)
eq "desktop after unlock: green pixels" "$(jnum "$post" green)" 921600
eq "desktop after unlock: distinct colours" "$(jnum "$post" distinct)" 1
stop_sway

# ================================================================ T2 =========
printf '\n== [2] SIGKILL while locked: the session must stay locked ==\n'
start_sway t2 || exit 1
OUT2="$OUT/t2"; mkdir -p "$OUT2"

R1_NESTED=1 "$BIN" --expect-outputs 2 --dump-dir "$OUT2" --hold 240 --label "R1 T2" \
    < /dev/null > "$RUNDIR/t2.jsonl" 2> "$RUNDIR/t2.err" &
CPID=$!
PIDS+=("$CPID")
wait_ready "$RUNDIR/t2.jsonl" 20 && pass "client locked and painted" || fail "client was not ready"
EVID="$OUT2"
before=$(shot locked-before-kill)
eq "before kill: green pixels" "$(jnum "$before" green)" 0
note "before kill: top=$(jstr "$before" top) (a lock frame)"

# Sample the output across the crash and for a second afterwards.
mkdir -p "$RUNDIR/caps"
start_capture "$RUNDIR/caps" HEADLESS-1
sleep 0.2
kill -9 "$CPID"; wait "$CPID" 2>/dev/null; rc=$?
eq "client died by SIGKILL" "$rc" 137
sleep 1.2
stop_capture
after=$(shot locked-after-kill)
read -r caps2 green2 late2 <<<"$(frame_stats "$RUNDIR/caps")"
ge "frames sampled across the crash" "$caps2" 1
eq "frames showing desktop content anywhere in the crash window" "${green2:-1}" 0
eq "desktop frames after the crash window started" "${late2:-1}" 0
eq "after kill: green pixels on HEADLESS-1" "$(jnum "$after" green)" 0
eq "after kill: sway's 'abandoned lock' backdrop on HEADLESS-1" "$(jstr "$after" top)" "#ff0000"
ge "after kill: red backdrop pixels" "$(jnum "$after" red)" 900000
note "after kill: $(jnum "$after" distinct) colour(s), see ${EVID#"$SPIKE"/}/locked-after-kill.png"
grep -q 'session lock abandoned' "$RUNDIR/sway.log" \
    && pass "sway log: 'session lock abandoned' (sway/lock.c handle_abandon keeps the lock)" \
    || fail "sway log has no 'session lock abandoned'"

printf -- '-- recovery with a new lock client (allowed by the protocol) --\n'
mkfifo "$RUNDIR/stdin-recover"
R1_NESTED=1 "$BIN" --expect-outputs 2 --dump-dir "$OUT2" --unlock-on-stdin --hold 120 \
    --label "R1 T2 recovery" < "$RUNDIR/stdin-recover" > "$RUNDIR/t2recover.jsonl" 2> "$RUNDIR/t2recover.err" &
RPID=$!
PIDS+=("$RPID")
exec 7> "$RUNDIR/stdin-recover"
wait_ready "$RUNDIR/t2recover.jsonl" 20 && pass "a replacement lock client is accepted and locks the session" \
    || fail "the replacement client never locked"
grep -q '"ev":"locked"' "$RUNDIR/t2recover.jsonl" && pass "replacement got 'locked' (not 'finished')" \
    || fail "replacement did not get 'locked'"
grep -q 'Replacing abandoned lock' "$RUNDIR/sway.log" \
    && pass "sway log: 'Replacing abandoned lock'" || note "no 'Replacing abandoned lock' in the log"
rec=$(shot recovered-lock)
eq "with the replacement locked: green pixels" "$(jnum "$rec" green)" 0
note "replacement frame: top=$(jstr "$rec" top)"
if kill -0 "$RPID" 2>/dev/null; then
    echo unlock >&7
else
    fail "replacement client died before the unlock request: $(tail -2 "$RUNDIR/t2recover.jsonl")"
fi
exec 7>&-
wait "$RPID"; rc=$?
eq "replacement client exit code" "$rc" 0
final=$(shot desktop-after-recovery)
eq "desktop after the replacement unlocked: green pixels" "$(jnum "$final" green)" 921600
stop_sway
note "SIGKILL did not unlock the session: it stayed locked with a red compositor backdrop"

# ================================================================ T3 =========
printf '\n== [3] resize while locked: reconfigure + repaint, no frame of desktop ==\n'
start_sway t3 || exit 1
OUT3="$OUT/t3"; mkdir -p "$OUT3"

mkfifo "$RUNDIR/stdin-resize"
R1_NESTED=1 "$BIN" --expect-outputs 2 --dump-dir "$OUT3" --unlock-on-stdin --hold 240 \
    --label "R1 T3" < "$RUNDIR/stdin-resize" > "$RUNDIR/t3.jsonl" 2> "$RUNDIR/t3.err" &
CPID=$!
PIDS+=("$CPID")
exec 7> "$RUNDIR/stdin-resize"
wait_ready "$RUNDIR/t3.jsonl" 20 && pass "locked at 1280x720 x2" || fail "client was not ready"
base=$(grep -c '"ev":"configure"' "$RUNDIR/t3.jsonl")

mkdir -p "$RUNDIR/caps"
start_capture "$RUNDIR/caps" HEADLESS-1
step() { # step <expected WxH> <swaymsg args...>
    local wh=$1; shift
    local n; n=$(($(grep -c '"ev":"configure"' "$RUNDIR/t3.jsonl") + 1))
    if timeout 10 "$SWAYMSG" "$@" > "$RUNDIR/last-swaymsg.txt" 2>&1 &&
       grep -q '"success": true' "$RUNDIR/last-swaymsg.txt"; then
        note "swaymsg $* -> ok, waiting for a $wh repaint"
    else
        fail "swaymsg $* failed: $(head -2 "$RUNDIR/last-swaymsg.txt")"
    fi
    if wait_last_size "$RUNDIR/t3.jsonl" HEADLESS-1 "$wh" 8; then
        pass "compositor reconfigured the lock surface to $wh and the client repainted"
    else
        fail "newest configure is $(last_size "$RUNDIR/t3.jsonl" HEADLESS-1), wanted $wh"
    fi
    wait_paints "$RUNDIR/t3.jsonl" "$n" 4 || true
    cp "$RUNDIR/last-swaymsg.txt" "$OUT3/swaymsg-$wh.txt" 2>/dev/null
}
step 2560x1440 output HEADLESS-1 resolution 2560x1440
EVID="$OUT3"
big=$(shot locked-2560x1440)
eq "at 2560x1440: green pixels" "$(jnum "$big" green)" 0
eq "at 2560x1440: captured frame size" "$(jnum "$big" w)x$(jnum "$big" h)" 2560x1440
step 1600x900 output HEADLESS-1 resolution 1600x900
step 800x450 output HEADLESS-1 scale 2
step 1600x900 output HEADLESS-1 scale 1
step 1280x720 output HEADLESS-1 resolution 1280x720
sleep 0.3
stop_capture

for wh in 2560x1440 1600x900 800x450; do
    line=$(grep "\"w\":${wh%x*},\"h\":${wh#*x}" "$RUNDIR/t3.jsonl" | grep '"ev":"configure"' | head -1)
    if [ -n "$line" ]; then
        png=$(jstr "$line" png)
        eq "buffer at $wh has exactly $(( ${wh%x*} * ${wh#*x} )) pixels" "$(jnum "$line" pixels)" "$(( ${wh%x*} * ${wh#*x} ))"
        [ -f "$png" ] && pass "PNG at $wh dumped: ${png#"$SPIKE"/}" || fail "no PNG at $wh"
        awk -v v="$(grep -o '"known_frac":[0-9.]*' <<<"$line" | cut -d: -f2)" \
            'BEGIN{exit !(v>=0.9999)}' && pass "frame at $wh fully painted" || fail "frame at $wh not fully painted"
    else
        fail "no configure/repaint at $wh"
    fi
done
ndone=$(grep -c '"ev":"configure"' "$RUNDIR/t3.jsonl")
ge "repaints over the whole resize sequence" "$ndone" $((base + 4))
eq "a different frame for every paint" \
    "$(grep '"ev":"configure"' "$RUNDIR/t3.jsonl" | grep -o '"hash":"[^"]*"' | sort -u | wc -l)" "$ndone"
grep -qE '"ev":"(protocol_error|fatal)"' "$RUNDIR/t3.jsonl" \
    && fail "protocol error during resize: $(grep -E '"ev":"(protocol_error|fatal)"' "$RUNDIR/t3.jsonl" | head -1)" \
    || pass "no protocol error (a buffer not matching the acked size would be dimensions_mismatch)"

grep -qE '"ev":"(protocol_error|fatal)"' "$RUNDIR/t3.jsonl" && fail "client reported: $(grep -E '"ev":"(protocol_error|fatal)"' "$RUNDIR/t3.jsonl" | head -2)"
read -r caps3 green3 late3 <<<"$(frame_stats "$RUNDIR/caps")"
ge "output samples captured during the resize sequence" "$caps3" 3
eq "samples with any desktop (green) pixel" "${green3:-1}" 0
eq "desktop frames after the first lock frame" "${late3:-1}" 0
first_ts=$(head -1 "$RUNDIR/caps.tsv" | cut -f1); last_ts=$(tail -1 "$RUNDIR/caps.tsv" | cut -f1)
if [ -n "${first_ts:-}" ] && [ -n "${last_ts:-}" ] && [ "$last_ts" -gt "$first_ts" ]; then
    span=$(( last_ts - first_ts ))
    note "$caps3 samples over ${span} ms (~$(( caps3 * 1000 / span )) Hz), none showed desktop content"
fi
note "the sizes above are the exact configure sizes; a mismatch would have killed the client"

if kill -0 "$CPID" 2>/dev/null; then
    echo unlock >&7
else
    fail "client died during the resize test: $(tail -3 "$RUNDIR/t3.jsonl")"
fi
exec 7>&-
wait "$CPID"; rc=$?
eq "client exit code after unlock" "$rc" 0
post=$(shot desktop-after-unlock)
eq "desktop after unlock: green pixels" "$(jnum "$post" green)" 921600
stop_sway

# ============================================================ summary =======
printf '\n== evidence ==\n'
printf '  frames + screenshots: %s\n' "${OUT#"$SPIKE"/}/"
for d in "$OUT"/*/; do
    printf '    %s %s files\n' "$(basename "$d")/" "$(ls "$d" | wc -l)"
done
printf '\n'
if [ "$FAIL" -eq 0 ]; then
    echo "RESULT: all assertions passed"
else
    echo "RESULT: some assertions FAILED (see above)"
fi
exit "$FAIL"
