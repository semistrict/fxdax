#!/usr/bin/env bash
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Checks fxjob's admission (scripts/fxjob) on a host set up for it (ubu1, its user's systemd
# manager with fxdax.slice), beside whatever jobs the host runs, which it neither counts nor
# touches: its jobs run in slices of its own under fxdax.slice, on a budget of their own (300M,
# 10 CPUs), with a queue, history and measurements' lock of their own in a temporary directory,
# and their measurements isolate nothing (FXJOB_ISOLATION=0). Each job is a sleep of a few
# seconds, memory reserved, none used; it takes about a minute.
#
#   scripts/fxjob-check.sh            checks scripts/fxjob
#   FXJOB=/usr/local/bin/fxjob scripts/fxjob-check.sh
#                                     checks the one installed
#   FXJOB_CHECK_OOM=1 scripts/fxjob-check.sh
#                                     checks a job past its limit too: an OOM kill of 64M, which
#                                     the desktop reports as the system out of memory
#
# What it checks: a request past the budget fails at once; jobs past the budget wait and start
# first come, first served, a small one never passing a large one ahead of it, as many at once
# as the budget holds, in memory and in CPUs; a job killed outright (kill -9) frees its
# reservation, and a waiting fxjob killed outright its place; a signal ends a waiting fxjob; a
# measurement waits for running work to leave it room, work after it waits behind it, and
# another measurement for it to end; a job started by another's command skips the queue; fxjob
# ps shows the waiting and the reservations without errors; every job that ran leaves a line of
# history, its status the command's, 137 for one its memory limit killed.
set -euo pipefail
repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
fxjob=${FXJOB:-$repo/scripts/fxjob}
tmp=$(mktemp -d "${TMPDIR:-/tmp}/fxjob-check.XXXXXX")
export FXJOB_CONF=$tmp/fxjob.conf FXJOB_RUNTIME=$tmp/run FXJOB_HISTORY=$tmp/jobs.jsonl
export FXJOB_WORK_SLICE=fxdax-fxjobcheck.slice FXJOB_BENCH_SLICE=fxdax-fxjobcheckbench.slice
export FXJOB_BENCH_LOCK=$tmp/bench.lock FXJOB_ISOLATION=0
printf 'MEM_BUDGET=300M\nCPU_BUDGET=10 # logical CPUs\n' > "$FXJOB_CONF"
log=$tmp/log
: > "$log"

# What is left of the checks' jobs goes, theirs alone: their slices' scopes.
finish() {
  local pids
  pids=$(jobs -p)
  if [[ -n $pids ]]; then kill -KILL $pids 2> /dev/null || true; fi
  systemctl --user stop "$FXJOB_WORK_SLICE" "$FXJOB_BENCH_SLICE" 2> /dev/null || true
  rm -rf "$tmp"
}
trap finish EXIT

failures=0
check() { # DESCRIPTION COMMAND...: says whether COMMAND succeeds
  if "${@:2}"; then
    echo "ok    $1"
  else
    echo "FAIL  $1"
    failures=$((failures + 1))
  fi
}

# job NAME SECONDS FXJOB_OPTION...: runs a job in the background that sleeps SECONDS, logging its
# start and end; its fxjob's pid is pid[NAME], its standard error $tmp/NAME.err.
declare -A pid
job() {
  local name=$1 seconds=$2
  shift 2
  "$fxjob" --name "$name" "$@" -- bash -c '
    echo "start $1 $EPOCHREALTIME" >> "$2"
    sleep "$3"
    echo "end $1 $EPOCHREALTIME" >> "$2"' job "$name" "$log" "$seconds" 2> "$tmp/$name.err" &
  pid[$name]=$!
  # Each job's turn is the order they came in: let it take its place before the next.
  sleep 0.3
}
# at EVENT NAME: when the job NAME started or ended.
at() { awk -v e="$1" -v n="$2" '$1 == e && $2 == n { print $3 }' "$log"; }
# before EVENT1 NAME1 EVENT2 NAME2: whether NAME1's EVENT1 came before NAME2's EVENT2.
before() {
  local a b
  a=$(at "$1" "$2") b=$(at "$3" "$4")
  [[ -n $a && -n $b ]] && awk -v a="$a" -v b="$b" 'BEGIN { exit !(a < b) }'
}
# order NAME...: whether the jobs NAME started in that order.
order() { [[ $(awk '$1 == "start" { print $2 }' "$log" | grep -xF -f <(printf '%s\n' "$@") |
  paste -sd ' ') == "$*" ]]; }
# most NAME...: how many of the jobs NAME ran at once, at most.
most() {
  awk '{ print $3, $1, $2 }' "$log" | sort -n | grep -wF -f <(printf '%s\n' "$@") |
    awk '$2 == "start" { n++; if (n > m) m = n } $2 == "end" { n-- } END { print m + 0 }'
}
# finished NAME...: waits for the jobs NAME; status[NAME] their statuses.
declare -A status
finished() {
  local name
  for name in "$@"; do
    status[$name]=0
    wait "${pid[$name]}" || status[$name]=$?
  done
}
entries() { find "$FXJOB_RUNTIME/queue" -type f 2> /dev/null | wc -l; }

echo "== a request past the budget"
started=$EPOCHSECONDS
rc=0; "$fxjob" --mem 400M -- true 2> "$tmp/err" || rc=$?
check "--mem 400M of 300M fails, status 2" test $rc -eq 2
check "saying it would never start" grep -q 'never start' "$tmp/err"
rc=0; "$fxjob" --cpus 11 -- true 2> "$tmp/err" || rc=$?
check "--cpus 11 of 10 fails, status 2" test $rc -eq 2
rc=0; FXJOB_CONF=<(echo CPU_BUDGET=4) "$fxjob" --bench --mem 1M -- true 2> "$tmp/err" || rc=$?
check "a measurement's 8 CPUs of 4 fails, status 2" test $rc -eq 2
check "at once, no place taken in the queue" test $((EPOCHSECONDS - started)) -le 2 -a "$(entries)" -eq 0

echo "== six jobs of 100M on 300M"
names=(n1 n2 n3 n4 n5 n6)
for name in "${names[@]}"; do job "$name" 2 --mem 100M --cpus 1; done
finished "${names[@]}"
check "start in the order they came" order "${names[@]}"
check "three at once at most" test "$(most "${names[@]}")" -eq 3
check "n4 starts once one of the first three ended" before end n1 start n4
check "n4 said it waited" grep -q 'waits' "$tmp/n4.err"

echo "== a small job behind a large one waits its turn"
job big1 3 --mem 200M --cpus 1
job big2 1 --mem 200M --cpus 1
job small 1 --mem 50M --cpus 1
"$fxjob" ps > "$tmp/ps" 2> "$tmp/ps.err" && rc=0 || rc=$?
check "fxjob ps succeeds, printing no errors" test $rc -eq 0 -a ! -s "$tmp/ps.err"
check "fxjob ps lists big2 first, then small, waiting" \
  grep -qzE 'big2-[0-9]+ +200M +1 .*small-[0-9]+ +50M +1 ' "$tmp/ps"
check "fxjob ps says what is reserved" grep -q 'reserved 200M of 300M, 1 of 10 CPUs, 1 jobs' "$tmp/ps"
finished big1 big2 small
check "small, though it fitted beside big1, starts after big2" order big1 big2 small
check "small starts once big1 ended" before end big1 start small

echo "== CPUs"
job c1 2 --mem 10M --cpus 6
job c2 1 --mem 10M --cpus 6
finished c1 c2
check "6 CPUs wait for 6 of 10 to end" before end c1 start c2

echo "== a job killed outright frees its reservation"
setsid "$fxjob" --name k1 --mem 250M --cpus 1 -- sleep 300 2> "$tmp/k1.err" &
pid[k1]=$!
sleep 1
job k2 1 --mem 100M --cpus 1
sleep 1
check "k2 waits while k1 runs" test -z "$(at start k2)"
killed=$EPOCHREALTIME
kill -KILL -- "-${pid[k1]}"
finished k1 k2
check "k1's job ended by its SIGKILL" test "${status[k1]}" -eq 137
check "k2 starts within 3 s of it" awk -v a="$killed" -v b="$(at start k2)" 'BEGIN { exit !(b - a < 3) }'
check "k1's scope is gone" bash -c "! systemctl --user list-units --plain --no-legend 'fxjob-k1-*' | grep -q ."

echo "== a waiting fxjob killed outright gives up its place"
job q1 3 --mem 250M --cpus 1
job q2 1 --mem 200M --cpus 1
job q3 1 --mem 50M --cpus 1
kill -KILL "${pid[q2]}"
finished q1 q2 q3
check "q3 starts beside q1, q2 gone" before start q3 end q1
check "q2 never ran" test -z "$(at start q2)"
check "the queue is empty" test "$(entries)" -eq 0

echo "== a signal ends a waiting fxjob"
job s1 2 --mem 250M --cpus 1
job s2 1 --mem 250M --cpus 1
kill -TERM "${pid[s2]}"
finished s1 s2
check "s2 exits 143, never having run" test "${status[s2]}" -eq 143 -a -z "$(at start s2)"
check "the queue is empty" test "$(entries)" -eq 0

echo "== measurements"
job w1 2 --mem 100M --cpus 4
job b1 2 --bench --mem 100M
job w2 1 --mem 50M --cpus 1
job b2 1 --bench --mem 50M
finished w1 b1 w2 b2
check "b1's 8 CPUs wait for w1's 4 of 10 to end" before end w1 start b1
check "w2, which fitted beside w1, waits behind b1" order w1 b1 w2
check "w2 runs beside b1" before start w2 end b1
check "b2 waits for b1 to end" before end b1 start b2
check "b2 said it waited for b1" grep -q 'another measurement' "$tmp/b2.err"

echo "== a job inside a job skips the queue"
rc=0
"$fxjob" --name outer --mem 250M --cpus 1 -- "$fxjob" --name inner --mem 100M --cpus 1 -- true \
  2> "$tmp/outer.err" || rc=$?
check "inner, past the budget beside outer, runs" test $rc -eq 0
check "saying it didn't wait its turn" grep -q 'without waiting' "$tmp/outer.err"

echo "== status and history"
rc=0; "$fxjob" --name three --mem 10M -- bash -c 'exit 3' || rc=$?
check "fxjob exits with the command's status" test $rc -eq 3
# A measurement here never swaps (MemorySwapMax=0), so its limit kills it rather than swapping it
# out on a host whose swap is someone else's. What it takes is bounded, 256M read into memory,
# its output nowhere: were the limit not set, it would take that and no more. (tail of /dev/zero,
# which was to grow without end, wrote the zeros out instead, and what read them, outside every
# job's limit, grew to 11.7 GB, until user-1000.slice's OOM killer took it.)
# Only with FXJOB_CHECK_OOM=1: the kernel's OOM kill, however small and contained, has the
# desktop say the system ran out of memory.
oom=${FXJOB_CHECK_OOM:-0}
if [[ $oom == 1 ]]; then
  rc=0
  "$fxjob" --name oom --bench --mem 64M -- \
    bash -c 'head -c 256M /dev/zero | python3 -c "import sys; d = sys.stdin.buffer.read()"' \
    > /dev/null 2>&1 || rc=$?
  check "a job past its memory limit exits 137" test $rc -eq 137
fi
python3 - "$FXJOB_HISTORY" > "$tmp/history" <<'EOF'
import json, sys
for line in open(sys.argv[1]):
    j = json.loads(line)
    print(j["name"], j["status"], j["mem"], j["cpus"], j["queued_s"], j["wall_s"],
          j["peak_mem"], j["peak_swap"], j["cpu_s"], str(j["bench"]).lower())
EOF
history() { awk -v n="$1" '$1 == n' "$tmp/history"; }
check "every line is JSON with every field" test -s "$tmp/history"
ran=(n1 n2 n3 n4 n5 n6 big1 big2 small c1 c2 k2 q1 q3 s1 w1 b1 w2 b2 inner outer three)
if [[ $oom == 1 ]]; then ran+=(oom); fi
check "a line for each job that ran, and no other" \
  test "$(cut -d' ' -f1 "$tmp/history" | sort | paste -sd ' ')" == \
  "$(printf '%s\n' "${ran[@]}" | sort | paste -sd ' ')"
check "n4's says it waited" awk '$1 == "n4" && $5 > 1 { f = 1 } END { exit !f }' "$tmp/history"
check "n1's says it didn't, ran 2 s, asked 100M and 1 CPU" \
  awk '$1 == "n1" && $5 < 1 && $6 >= 2 && $3 == 104857600 && $4 == 1 { f = 1 } END { exit !f }' "$tmp/history"
check "three's status is 3" test "$(history three | cut -d' ' -f2)" == 3
if [[ $oom == 1 ]]; then
  check "oom's status is 137, its peak its limit" \
    awk '$1 == "oom" && $2 == 137 && $7 >= 60 * 2^20 && $10 == "true" { f = 1 } END { exit !f }' "$tmp/history"
fi
check "k1's, killed with its fxjob, has none" test -z "$(history k1)"
check "peaks and CPU time measured" \
  awk '$7 == "null" || $8 == "null" || $9 == "null" { f = 1 } END { exit f }' "$tmp/history"

echo
if (( failures )); then
  echo "fxjob-check: $failures failed" >&2
  exit 1
fi
echo "fxjob-check: all passed"
