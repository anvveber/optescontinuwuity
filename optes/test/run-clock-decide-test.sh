#!/usr/bin/env bash
# The decision rule of optes-clock.sh, with fixed samples ("offset rtt" in
# seconds). Needs nothing but bash; changes nothing.
# Usage: run-clock-decide-test.sh
set -uo pipefail

tool=$(cd "$(dirname "$0")/.." && pwd)/optes-clock.sh
fail=0

expect() {
  local name=$1 want=$2 input=$3 got
  got=$(printf '%b' "$input" | ALLOW_BACK=${ALLOW_BACK:-no} "$tool" decide)
  if [[ $got == "$want"* ]]; then
    echo "PASS $name  ($got)"
  else
    echo "FAIL $name: want '$want…', got '$got'"
    fail=1
  fi
}

expect "no samples"                "refuse only 0"  ""
expect "two samples are too few"   "refuse only 2"  "30 1\n31 1\n"
expect "slow samples do not count" "refuse only 2"  "30 1\n31 2\n300 7\n"
expect "in the dead band"          "ok 3"           "2 1\n3 1\n4 2\n"
expect "dead band edge"            "ok -10"         "-10 1\n-10 1\n-9 1\n"
expect "just outside the band"     "step 11"        "11 1\n11 1\n12 1\n"
expect "minutes behind"            "step 601"       "600 1\n601 2\n603 5\n"
expect "30 min ahead"              "step -1800"     "-1801 1\n-1800 1\n-1799 1\n"
expect "hours ahead: refused"      "refuse ahead by 7200" "-7201 3\n-7200 1\n-7199 2\n"
ALLOW_BACK=yes expect "hours ahead, --allow-back" "step -7200" "-7201 3\n-7200 1\n-7199 2\n"
expect "one day behind"            "step 86400"     "86400 1\n86401 1\n86399 1\n"
expect "more than a day"           "refuse off by 86402" "86402 1\n86402 1\n86403 1\n"
expect "sources disagree"          "refuse sources disagree" "600 1\n601 1\n620 1\n"
expect "one far outlier is slow"   "step 601"       "600 1\n601 1\n602 1\n9999 30\n"
expect "malformed lines ignored"   "step 600"       "600 1\nx 1\n601 y\n\n600 2\n-- 1\n599 1\n"
expect "median of an even count"   "step 601"       "600 1\n600 1\n601 1\n602 1\n"

if (( fail )); then echo "SOME FAILED"; exit 1; fi
echo "ALL PASSED"
