#!/usr/bin/env bash
# spike R6 — KDL binds generator for Niri.
# One command reproduces every claim in RESULT.md:
#   ./run.sh
# Works both inside and outside `nix develop` (it re-execs itself in the devShell).
set -uo pipefail
self="$(readlink -f "${BASH_SOURCE[0]}")"   # before the cd: the argument may be relative
cd "$(dirname "$self")" || exit 1

if ! command -v cargo >/dev/null 2>&1; then
    echo "[run.sh] no cargo in PATH, re-exec inside the shell devShell"
    exec nix develop /home/nezerkill/shell --no-warn-dirty -c bash "$self" "$@"
fi

FAIL=0
pass() { printf '  PASS  %s\n' "$1"; }
fail() { printf '  FAIL  %s\n' "$1"; FAIL=1; }
note() { printf '  note  %s\n' "$1"; }
# Every niri invocation runs with no session env, which also proves `validate`
# does not need a running Wayland compositor.
run_niri() { env -u WAYLAND_DISPLAY -u DISPLAY -u WAYLAND_SOCKET -u NIRI_SOCKET -u NIRI_CONFIG "$NIRI_BIN" "$@"; }

# --- niri: native binary, or fetch it from nixpkgs ---------------------------
echo "== locate niri =="
NIRI_BIN=""
if command -v niri >/dev/null 2>&1; then
    NIRI_BIN="$(command -v niri)"
else
    echo "  no niri in PATH, trying \`nix build --no-link --print-out-paths nixpkgs#niri\`"
    if store_path=$(nix build --no-link --print-out-paths nixpkgs#niri 2>/dev/null) \
        && [ -n "$store_path" ] && [ -x "$store_path/bin/niri" ]; then
        NIRI_BIN="$store_path/bin/niri"
    fi
fi
if [ -z "$NIRI_BIN" ] || [ ! -x "$NIRI_BIN" ]; then
    echo
    echo "[run.sh] SKIP: niri is not installed and could not be fetched from the network."
    echo "[run.sh]    (nothing below ran; this is a skip, not a pass)"
    echo "RESULT: SKIPPED (niri unavailable)"
    exit 0
fi
printf '  niri at %s\n  %s\n' "$NIRI_BIN" "$(run_niri --version 2>&1 | head -1)"
run_niri validate --help 2>&1 | sed -n '1,3p' | sed 's/^/    | /'

# --- build the generator -----------------------------------------------------
echo
echo "== build r6-binds (offline, no dependencies) =="
cargo build --offline -q --manifest-path Cargo.toml || { echo "generator build FAILED"; exit 1; }
printf '  rustc %s\n' "$(rustc --version | cut -d' ' -f2)"
cargo test --offline -q --manifest-path Cargo.toml 2>&1 | grep -E '^test result' | sed 's/^/  unit tests: /'

GEN="./target/debug/r6-binds"
rm -rf out
mkdir -p out/include

# --- generate ----------------------------------------------------------------
echo
echo "== generate =="
"$GEN" gen specs/minimal.json out/minimal.kdl || fail "generator failed on specs/minimal.json"
"$GEN" gen specs/full.json out/full.kdl || fail "generator failed on specs/full.json"
"$GEN" gen specs/bad-action.json out/bad-action.kdl && note "bad-action.kdl emitted on purpose (niri must reject it)"
echo "  --- out/minimal.kdl ---"
sed 's/^/    | /' out/minimal.kdl
echo "  --- out/full.kdl ---"
sed 's/^/    | /' out/full.kdl

# --- [1] minimal fragment ----------------------------------------------------
echo
echo "== [1] niri validate -c out/minimal.kdl =="
out=$(run_niri validate -c out/minimal.kdl 2>&1); rc=$?
echo "$out" | sed 's/^/    | /'
[ "$rc" -eq 0 ] && pass "exit code 0" || fail "exit code $rc (want 0)"
grep -qi 'error' <<<"$out" && fail "output mentions an error" || pass "no error in output"

# --- [2] full fragment -------------------------------------------------------
echo
echo "== [2] niri validate -c out/full.kdl (every supported action shape) =="
out=$(run_niri validate -c out/full.kdl 2>&1); rc=$?
echo "$out" | sed 's/^/    | /'
[ "$rc" -eq 0 ] && pass "exit code 0" || fail "exit code $rc (want 0)"
grep -qi 'error' <<<"$out" && fail "output mentions an error" || pass "no error in output"
grep -q 'Mod+Return hotkey-overlay-title="Терминал" { spawn "foot"; }' < out/full.kdl \
    && pass "spawn bind emitted" || fail "no spawn bind"
grep -q 'Mod+D { spawn "shellctl" "call" "launcher.toggle"; }' < out/full.kdl \
    && pass "shell IPC bind is a spawn of shellctl" || fail "no ipc bind"
grep -q 'Mod+2 { focus-workspace 2; }' < out/full.kdl && pass "niri action with integer argument" || fail "no focus-workspace bind"
grep -q 'Mod+Plus { set-column-width "+10%"; }' < out/full.kdl && pass "niri action with quoted argument" || fail "no set-column-width bind"
grep -q 'Mod+Shift+Slash hotkey-overlay-title=null' < out/full.kdl && pass "hotkey-overlay-title=null" || fail "no null title"
grep -q 'repeat=false' < out/full.kdl && pass "repeat=false property" || fail "no repeat property"
grep -q 'cooldown-ms=150' < out/full.kdl && pass "cooldown-ms property" || fail "no cooldown-ms property"
grep -q 'allow-when-locked=true { spawn "swaylock"; }' < out/full.kdl && pass "allow-when-locked on a spawn bind" || fail "no allow-when-locked bind"
grep -q '^hotkey-overlay {' < out/full.kdl && pass "hotkey-overlay { skip-at-startup } block emitted" || fail "no hotkey-overlay block"
grep -q 'XF86AudioMicMute' < out/full.kdl && pass "media key without modifiers emitted" || fail "no XF86 bind"
grep -q 'SUPER' < out/full.kdl && fail "SUPER leaked into the fragment (alias broken)" || pass "SUPER aliased to Mod everywhere"
grep -q 'Терминал' < out/full.kdl && pass "UTF-8 overlay title survives" || fail "UTF-8 title lost"

# --- [3] negative: bad action name ------------------------------------------
echo
echo "== [3] control: generated fragment with an unknown niri action must FAIL =="
out=$(run_niri validate -c out/bad-action.kdl 2>&1); rc=$?
grep -E 'Error:|invalid|expected' <<<"$out" | head -3 | sed 's/^/    | /'
[ "$rc" -ne 0 ] && pass "exit code $rc (non-zero, as required)" || fail "exit code 0 — validate is not really checking the file"
grep -qi 'error' <<<"$out" && pass "output reports an error" || fail "no error text in output"

# --- [4] negative: duplicate keybind (hand-written fixture) -----------------
echo
echo "== [4] control: duplicate keybind in fixtures/duplicate.kdl must FAIL =="
out=$(run_niri validate -c fixtures/duplicate.kdl 2>&1); rc=$?
grep -E 'Error:|duplicate keybind' <<<"$out" | head -3 | sed 's/^/    | /'
[ "$rc" -ne 0 ] && pass "exit code $rc (non-zero, as required)" || fail "exit code 0 — niri accepted a duplicate bind"
grep -q 'duplicate keybind' <<<"$out" && pass "niri names the duplicate" || fail "no duplicate diagnosis"

# --- [5] our own conflict detection (niri rejects reordered modifiers too) ---
echo
echo "== [5] generator refuses a spec that niri would reject as a duplicate =="
rm -f out/duplicate.kdl
out=$("$GEN" gen specs/duplicate.json out/duplicate.kdl 2>&1); rc=$?
echo "$out" | sed 's/^/    | /'
[ "$rc" -eq 2 ] && pass "exit code 2 (invalid spec)" || fail "exit code $rc (want 2)"
grep -q 'duplicate keybind' <<<"$out" && pass "modifier order normalised before the check" || fail "no duplicate diagnosis"
[ -f out/duplicate.kdl ] && fail "a fragment was written despite the conflict" || pass "no file written"

# --- [6] niri's own rule: allow-when-locked only on spawn -------------------
echo
echo "== [6] generator refuses allow_when_locked on a niri action =="
out=$("$GEN" gen specs/locked-action.json out/locked.kdl 2>&1); rc=$?
echo "$out" | sed 's/^/    | /'
[ "$rc" -eq 2 ] && pass "exit code 2" || fail "exit code $rc (want 2)"
grep -q 'only allowed on spawn' <<<"$out" && pass "message mirrors niri's rule" || fail "wrong message"

# --- [7] the syntax reference: niri's built-in default config --------------
echo
echo "== [7] syntax reference: the default config embedded in the niri binary =="
ref_start=$(grep -abo '// This config is in the KDL format' "$NIRI_BIN" | head -1 | cut -d: -f1)
ref_end=$(grep -abo 'default-config.kdl' "$NIRI_BIN" | head -1 | cut -d: -f1)
if [ -n "${ref_start:-}" ] && [ -n "${ref_end:-}" ] && [ "$ref_end" -gt "$ref_start" ]; then
    dd if="$NIRI_BIN" bs=1 skip="$ref_start" count=$((ref_end - ref_start)) status=none > out/niri-default-config.kdl
    printf '  extracted %s bytes (%s lines) from the binary\n' \
        "$(stat -c %s out/niri-default-config.kdl)" "$(wc -l < out/niri-default-config.kdl)"
    grep -m2 -E '^(binds|hotkey-overlay) \{' out/niri-default-config.kdl | sed 's/^/    | /'
    grep -m1 'allow-when-locked=true { spawn-sh' out/niri-default-config.kdl | sed 's/^/    | /'
    out=$(run_niri validate -c out/niri-default-config.kdl 2>&1); rc=$?
    [ "$rc" -eq 0 ] && pass "niri accepts its own default config => the extracted syntax is real" \
        || fail "the extracted default config does not validate (bad extraction)"
else
    note "could not locate the embedded default config, skipped"
fi

# --- [8] include: how a fragment gets wired in -----------------------------
echo
echo "== [8] include \"shell-binds.kdl\" from a real config =="
cp out/full.kdl out/include/shell-binds.kdl
printf 'input {\n    keyboard {\n        xkb {\n            layout "us,ru"\n        }\n    }\n}\n\ninclude "shell-binds.kdl"\n' > out/include/config.kdl
out=$(run_niri validate -c out/include/config.kdl 2>&1); rc=$?
[ "$rc" -eq 0 ] && pass "relative include resolves against the including file (exit 0)" || fail "exit $rc"
grep -qi 'error' <<<"$out" && fail "output mentions an error" || pass "no error in output"

# --- [9] cross-file bind conflict is NOT caught by niri --------------------
echo
echo "== [9] gap: a duplicate bind across include is NOT detected by niri =="
printf 'binds {\n    Mod+Return { spawn "user-terminal"; }\n}\ninclude "shell-binds.kdl"\n' > out/include/conflict.kdl
out=$(run_niri validate -c out/include/conflict.kdl 2>&1); rc=$?
[ "$rc" -eq 0 ] && note "niri accepts our Mod+Return next to the user's own Mod+Return (exit 0) => the shell must detect cross-file conflicts itself" \
    || note "niri rejected the cross-file duplicate (exit $rc) => recheck this claim"
printf 'include "does-not-exist.kdl"\n' > out/include/missing.kdl
out=$(run_niri validate -c out/include/missing.kdl 2>&1); rc=$?
[ "$rc" -ne 0 ] && pass "a missing include is an error (exit $rc)" || fail "missing include passed silently"

# --- summary -----------------------------------------------------------------
echo
if [ "$FAIL" -eq 0 ]; then
    echo "RESULT: all assertions passed"
else
    echo "RESULT: some assertions FAILED (see above)"
fi
exit "$FAIL"
