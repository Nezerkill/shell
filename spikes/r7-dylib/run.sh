#!/usr/bin/env bash
# R7 spike — dylib plugin failure modes.
# One command reproduces every number quoted in RESULT.md:
#   ./run.sh
# Works both inside and outside `nix develop` (it re-execs itself in the shell).
set -uo pipefail
cd "$(dirname "$(readlink -f "${BASH_SOURCE[0]}")")" || exit 1

if ! command -v cargo >/dev/null 2>&1; then
    echo "[run.sh] no cargo in PATH, re-exec inside the shell devShell"
    exec nix develop /home/nezerkill/shell --no-warn-dirty -c bash "$(readlink -f "${BASH_SOURCE[0]}")" "$@"
fi

ulimit -c 0 # the segfault/abort cases must not litter core files

HOST=host/target/release/r7-host
SO=plugin/target/release/libr7_plugin.so
FAIL=0

pass() { printf '  PASS  %s\n' "$1"; }
fail() { printf '  FAIL  %s\n' "$1"; FAIL=1; }
note() { printf '  note  %s\n' "$1"; }

# --- build ------------------------------------------------------------------
echo "== build (release) =="
cargo build --offline --release --manifest-path plugin/Cargo.toml -q || { echo "plugin build FAILED"; exit 1; }
cargo build --offline --release --manifest-path host/Cargo.toml -q || { echo "host build FAILED"; exit 1; }
printf '  rustc %s / cargo %s\n' "$(rustc --version | cut -d' ' -f2)" "$(cargo --version | cut -d' ' -f2)"
printf '  %s = %s bytes (%s)\n' "$SO" "$(stat -c %s "$SO")" "$(du -h "$SO" | cut -f1)"
printf '  %s = %s bytes\n' "$HOST" "$(stat -c %s "$HOST")"

# --- ok ---------------------------------------------------------------------
echo
echo "== [1] R7_MODE=ok =="
out=$(R7_MODE=ok "$HOST" "$SO" 2>&1); rc=$?
echo "$out" | sed 's/^/    | /'
[ "$rc" -eq 0 ] && pass "exit code 0" || fail "exit code $rc (want 0)"
grep -q 'result: Ok("pong:payload|method=7")' <<<"$out" && pass "Ok result returned" || fail "no Ok result"

# --- panic, caught inside the plugin ----------------------------------------
echo
echo "== [2] R7_MODE=panic (plugin catches at its own ABI boundary) =="
out=$(R7_MODE=panic "$HOST" "$SO" 2>&1); rc=$?
grep -E '^(host|plugin):|spike panic' <<<"$out" | head -6 | sed 's/^/    | /'
[ "$rc" -eq 0 ] && pass "exit code 0, host survived" || fail "exit code $rc (want 0)"
grep -q 'result: Err("caught panic at ABI boundary: spike panic")' <<<"$out" \
    && pass "panic converted into Err(...)" || fail "panic not reported as Err"
grep -q 'still alive' <<<"$out" && pass "host reached the end of main" || fail "host did not reach the end"

# --- panic, no manual catch_unwind: what does the ABI boundary itself do? ----
echo
echo "== [3] R7_MODE=panic R7_RAW=1 (no catch_unwind: bare extern \"C\" fn) =="
out=$(R7_RAW=1 R7_MODE=panic "$HOST" "$SO" 2>&1); rc=$?
grep -E 'panic in a function that cannot unwind|caused non-unwinding panic|^host:' <<<"$out" | head -5 | sed 's/^/    | /'
[ "$rc" -gt 128 ] && pass "process died on the panic (exit $rc) => abi_stable does NOT catch it for you" \
    || fail "process survived (exit $rc) - unexpected, abi_stable wrapped the call"
grep -q '^host: result:' <<<"$out" && fail "host printed a result after an uncaught panic" || pass "no result printed"

# --- abort ------------------------------------------------------------------
echo
echo "== [4] R7_MODE=abort =="
out=$(R7_MODE=abort "$HOST" "$SO" 2>&1); rc=$?
echo "$out" | sed 's/^/    | /'
[ "$rc" -eq 134 ] && pass "SIGABRT (exit 134), process gone" || fail "exit $rc (want 134)"
grep -q '^host: result:' <<<"$out" && fail "a result was printed before the abort" || pass "no result printed"

# --- segv -------------------------------------------------------------------
echo
echo "== [5] R7_MODE=segv =="
out=$(R7_MODE=segv "$HOST" "$SO" 2>&1); rc=$?
echo "$out" | sed 's/^/    | /'
[ "$rc" -eq 139 ] && pass "SIGSEGV (exit 139), process gone" || fail "exit $rc (want 139)"
grep -q '^host: result:' <<<"$out" && fail "a result was printed before the fault" || pass "no result printed"

# --- oom (address space capped so it fails fast and safely) -----------------
echo
echo "== [6] R7_MODE=oom (ulimit -v 2000000) =="
out=$( ( ulimit -v 2000000; R7_MODE=oom "$HOST" "$SO" ) 2>&1 ); rc=$?
grep -E 'memory allocation of|allocated chunk|aborting on purpose' <<<"$out" | tail -3 | sed 's/^/    | /'
[ "$rc" -eq 134 ] && pass "SIGABRT (exit 134) on allocation failure" || fail "exit $rc (want 134)"
grep -q '^host: result:' <<<"$out" && fail "a result was printed" || pass "no result printed"

# --- api_mismatch -----------------------------------------------------------
echo
echo "== [7] R7_MODE=api_mismatch (plugin reports plugin_api=999) =="
out=$(R7_MODE=api_mismatch "$HOST" "$SO" 2>&1); rc=$?
echo "$out" | sed 's/^/    | /'
[ "$rc" -eq 3 ] && pass "host exits 3, non-zero" || fail "exit $rc (want 3)"
grep -q 'REFUSING to load plugin' <<<"$out" && pass "clear error printed" || fail "no refusal message"
grep -q 'calling call(' <<<"$out" && fail "plugin body was called despite the mismatch" || pass "plugin body never called"

# --- independent plugin rebuild ---------------------------------------------
echo
echo "== [8] rebuild the plugin alone, host untouched =="
host_before=$(sha256sum "$HOST" | cut -d' ' -f1)
so_before=$(sha256sum "$SO" | cut -d' ' -f1)
cp plugin/src/lib.rs /tmp/r7-plugin-lib.rs.bak
restore() { cp /tmp/r7-plugin-lib.rs.bak plugin/src/lib.rs; rm -f /tmp/r7-plugin-lib.rs.bak; }
trap restore EXIT INT TERM
# Real code change, no ABI change: the answer text is renamed.
sed -i 's/|method={method}/|v2_method={method}/' plugin/src/lib.rs
cargo build --offline --release --manifest-path plugin/Cargo.toml -q || fail "plugin rebuild failed"
out=$(R7_MODE=ok "$HOST" "$SO" 2>&1); rc=$?
restore
cargo build --offline --release --manifest-path plugin/Cargo.toml -q # leave the .so matching the pristine source
trap - EXIT INT TERM
host_after=$(sha256sum "$HOST" | cut -d' ' -f1)
so_after=$(sha256sum "$SO" | cut -d' ' -f1)
printf '    | host sha256 %s -> %s (unchanged)\n' "${host_before:0:16}" "${host_after:0:16}"
printf '    | plugin sha256 %s -> %s (rebuilt)\n' "${so_before:0:16}" "${so_after:0:16}"
printf '    | plugin said: %s\n' "$(grep 'result:' <<<"$out" | sed 's/^host: //')"
[ "$host_before" = "$host_after" ] && pass "host binary untouched (never recompiled)" || fail "host binary changed"
[ "$rc" -eq 0 ] && pass "recompiled plugin loads into the unchanged host (exit 0)" || fail "exit $rc after rebuild"
grep -q 'v2_method=7' <<<"$out" && pass "the new plugin code actually ran (proves a real recompile)" || fail "old code ran"

# --- what abi_stable says about panics (from the vendored source) -----------
echo
echo "== [9] abi_stable 0.11.3 source: the only panic helper aborts =="
ABI_SRC=$(ls -d "$HOME"/.cargo/registry/src/*/abi_stable-0.11.3 2>/dev/null | head -1)
if [ -n "${ABI_SRC:-}" ]; then
    sed -n '/This macro causes an abort if a panic reaches this point/,+2p' "$ABI_SRC/src/macros.rs" | sed 's/^/    | /'
    sed -n '297,304p' "$ABI_SRC/src/library.rs" | grep -n 'catch_unwind\|Unwound' | sed 's/^/    | /'
else
    note "abi_stable sources not in the cargo registry cache, skipped"
fi

# --- summary ----------------------------------------------------------------
echo
if [ "$FAIL" -eq 0 ]; then
    echo "RESULT: all assertions passed"
else
    echo "RESULT: some assertions FAILED (see above)"
fi
exit "$FAIL"
