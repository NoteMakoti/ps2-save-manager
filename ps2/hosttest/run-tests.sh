#!/bin/sh
# Runs the PS2 server code (built for the host by `make`) against the
# desktop CLI: a normal round trip, a large save, a slow card, and a card
# whose driver stops answering (the server must stay responsive and
# report an error instead of hanging).
#
# usage: ps2/hosttest/run-tests.sh path/to/ps2sm path/to/interop
set -eu
PS2SM=${1:-target/debug/ps2sm}
INTEROP=${2:-target/debug/examples/interop}
HOST=$(cd "$(dirname "$0")" && pwd)/ps2sm-host
WORK=$(mktemp -d)
PIDS=""
cleanup() { for p in $PIDS; do kill "$p" 2>/dev/null || true; done; rm -rf "$WORK"; }
trap cleanup EXIT INT TERM
export RUST_BACKTRACE=0
BASE=$((20000 + $$ % 20000)) # fresh ports each run
P1=$BASE; P2=$((BASE + 1)); P3=$((BASE + 2))

start() { # port env...
    port=$1; shift
    mkdir -p "$WORK/$port/slot1"
    (cd "$WORK/$port" && env CARD_ROOT="$WORK/$port" "$@" "$HOST" "$port" >"$WORK/$port.log" 2>&1) &
    PIDS="$PIDS $!"
    i=0
    while ! grep -q "host test server" "$WORK/$port.log" 2>/dev/null; do
        i=$((i + 1)); [ $i -lt 50 ] || { cat "$WORK/$port.log"; fail "server on $port did not start"; }
        sleep 0.1
    done
}
fail() { echo "FAIL: $*"; exit 1; }

echo "== round trip"
start $P1
"$INTEROP" mksave "$WORK/s.psu" psu
"$PS2SM" ps2 127.0.0.1:$P1 put "$WORK/s.psu" >/dev/null
"$PS2SM" ps2 127.0.0.1:$P1 saves | grep -q BESLES-99999 || fail "save not listed"
"$PS2SM" ps2 127.0.0.1:$P1 get BESLES-99999 -o "$WORK/out" >/dev/null
[ "$("$INTEROP" dumpsave "$WORK/s.psu" | sort)" = "$("$INTEROP" dumpsave "$WORK/out/BESLES-99999.psu" | sort)" ] || fail "save changed in transfer"
"$PS2SM" ps2 127.0.0.1:$P1 dump "$WORK/d.ps2" >/dev/null
"$PS2SM" check "$WORK/d.ps2" | grep -q "no problems" || fail "dumped card has problems"

echo "== slow card, large save"
start $P2 MC_DELAY_MS=3
head -c 2000000 /dev/urandom >"$WORK/big.bin"
"$PS2SM" ps2 127.0.0.1:$P2 put "$WORK/s.psu" >/dev/null
"$PS2SM" ps2 127.0.0.1:$P2 add BESLES-99999 "$WORK/big.bin" >/dev/null
"$PS2SM" ps2 127.0.0.1:$P2 extract BESLES-99999 big.bin -o "$WORK/x" >/dev/null
cmp -s "$WORK/big.bin" "$WORK/x/big.bin" || fail "large file changed in transfer"

echo "== stuck memory card driver"
start $P3 MC_STUCK_AFTER=1
t0=$(date +%s)
if "$PS2SM" ps2 127.0.0.1:$P3 saves >"$WORK/stuck.out" 2>&1; then fail "stuck card reported success"; fi
t1=$(date +%s)
grep -q "did not respond in time" "$WORK/stuck.out" || { cat "$WORK/stuck.out"; fail "no timeout message"; }
[ $((t1 - t0)) -lt 15 ] || fail "took $((t1 - t0)) s to report a stuck card"
# the server must still answer
"$PS2SM" ps2 127.0.0.1:$P3 status >"$WORK/status.out" 2>&1 || true
grep -q "PS2SM on PlayStation 2" "$WORK/status.out" || { cat "$WORK/status.out"; fail "server stopped answering"; }
grep -q "still busy" "$WORK/status.out" || { cat "$WORK/status.out"; fail "busy driver not reported"; }

echo "all host tests passed"
