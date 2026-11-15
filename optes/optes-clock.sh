#!/usr/bin/env bash
# optes-clock.sh — keep the clock of a core or front roughly right, over Tor.
#
# Core and fronts have no NTP: the firewall lets only Tor out. This tool asks
# a few onion services for the time (the HTTP "Date" header, plain HTTP: the
# onion address authenticates the server, so no certificate and no clock is
# needed to trust the answer) and steps the clock when they agree that it is
# off. setup-core.sh and setup-front.sh install it with a timer.
#
# Written 2026-11-15.
#
# Usage (as root):
#   optes-clock.sh check [--allow-back]
#                                   ask the sources; step the clock if needed
#   optes-clock.sh status           print the result of the last check
#   optes-clock.sh decide           (tests) read "offset rtt" lines on stdin,
#                                   print the decision; changes nothing
#
# Rules:
#   - One sample per source, all sources at once: a first request warms the
#     circuit (a second one if it gets nothing), then up to two timed
#     requests; the fastest counts. A sample counts only if its round
#     trip took at most MAX_RTT s, since the error is up to half of it.
#   - At least MIN_SAMPLES samples, and they must agree within MAX_SPREAD s.
#   - The median offset is applied only if it is more than DEAD_BAND s and at
#     most MAX_STEP s. Beyond that the tool warns and leaves the clock alone.
#   - Back (the clock was ahead) at most MAX_BACK s, unless --allow-back.
#     Why: a Tor onion service numbers its descriptors by its clock, and Tor's
#     directories refuse a lower number than one they already hold. After a
#     step back, the onion's new descriptors are refused until real time
#     passes the clock time of its last upload: up to the size of the step.
#     The old descriptor keeps working, so this matters only if Tor restarts
#     (or `remove-client` runs) in that time; then the onion is unreachable
#     until that moment. Steps forward are safe. (Tested in a VM, 2026-11-15.)
#   - Never more than an hour earlier than the start of the consensus Tor has
#     on disk: that document is signed by Tor's directory authorities, so it
#     can't be much from the future.
# Exit status: 0 done; 1 no usable answer or sources disagree (the timer's
# unit retries in 20 minutes); 3 refused by a limit above (no retry: that
# waits for the operator).
# Sources: /etc/optes-clock/sources, one onion address per line.
# Result of the last check: /run/optes-clock/last (memory only).
#
# What can't be fixed here: a clock so far off that Tor can't connect (in
# tests: 2 days or more ahead). Then no source answers, status says so, and
# the clock must be set by hand on the provider's console.

set -euo pipefail
umask 077

SOURCES=/etc/optes-clock/sources
STATE_DIR=/run/optes-clock
LAST=$STATE_DIR/last
SOCKS=127.0.0.1:9050
CONSENSUS=/var/lib/tor/cached-microdesc-consensus

MAX_RTT=6
MIN_SAMPLES=3
MAX_SPREAD=8
DEAD_BAND=10
MAX_STEP=86400
MAX_BACK=1800
ALLOW_BACK=${ALLOW_BACK:-no}

die() { echo "ERROR: $*" >&2; exit 1; }

# Reads "offset rtt" lines (seconds, integers). Prints one line:
#   ok <median>        apply nothing, within the dead band
#   step <median>      step the clock by <median> seconds
#   refuse <reason>    leave the clock alone and warn
decide() {
  local offsets=() offset rtt
  while read -r offset rtt; do
    [[ $offset =~ ^-?[0-9]+$ && $rtt =~ ^[0-9]+$ ]] || continue
    (( rtt <= MAX_RTT )) || continue
    offsets+=("$offset")
  done
  local n=${#offsets[@]}
  if (( n < MIN_SAMPLES )); then
    echo "refuse only $n usable answer(s), need $MIN_SAMPLES"
    return
  fi
  mapfile -t offsets < <(printf '%s\n' "${offsets[@]}" | sort -n)
  local low=${offsets[0]} high=${offsets[n-1]}
  if (( high - low > MAX_SPREAD )); then
    echo "refuse sources disagree: offsets from $low to $high s"
    return
  fi
  local median=${offsets[n/2]}
  local size=${median#-}
  if (( size <= DEAD_BAND )); then
    echo "ok $median"
  elif (( size > MAX_STEP )); then
    echo "refuse off by $median s, more than the $MAX_STEP s this tool corrects"
  elif (( median < -MAX_BACK )) && [[ $ALLOW_BACK != yes ]]; then
    echo "refuse ahead by ${size} s; stepping back more than $MAX_BACK s needs --allow-back (see optes-clock.sh)"
  else
    echo "step $median"
  fi
}

# Unix time of the "Date" header of http://<onion>/, or nothing.
# The header comes from the network: only an exact RFC 7231 date is accepted.
fetch_date() {
  local onion=$1 header
  # --proxy beats proxy variables in the environment; --noproxy '' makes curl
  # ignore no_proxy, so the request can only go through Tor.
  header=$(curl -s -I --max-time 90 --proxy "socks5h://$SOCKS" --noproxy '' "http://$onion/" 2>/dev/null \
    | tr -d '\r' | sed -n 's/^[Dd][Aa][Tt][Ee]: //p' | head -n 1) || true
  [[ $header =~ ^(Mon|Tue|Wed|Thu|Fri|Sat|Sun),\ [0-9]{2}\ (Jan|Feb|Mar|Apr|May|Jun|Jul|Aug|Sep|Oct|Nov|Dec)\ [0-9]{4}\ [0-9]{2}:[0-9]{2}:[0-9]{2}\ GMT$ ]] || return 0
  date -u -d "$header" +%s
}

# Prints "offset rtt" for one source, or nothing.
sample() {
  local onion=$1 best_rtt="" best_offset="" start end server rtt
  # Warm the circuit; the first connection to an onion is often slow. If even a
  # second try gets nothing, give up on this source.
  [[ -n $(fetch_date "$onion") || -n $(fetch_date "$onion") ]] || return 0
  for _ in 1 2; do
    start=$(date +%s%N)
    server=$(fetch_date "$onion")
    end=$(date +%s%N)
    [[ -n $server ]] || continue
    rtt=$(( (end - start + 500000000) / 1000000000 ))
    if [[ -z $best_rtt ]] || (( rtt < best_rtt )); then
      best_rtt=$rtt
      best_offset=$(( server - (start + end) / 2000000000 ))
    fi
    (( rtt <= MAX_RTT )) && break
  done
  [[ -n $best_rtt ]] && echo "$best_offset $best_rtt"
  return 0
}

consensus_start() {
  [[ -r $CONSENSUS ]] || return 0
  local line
  line=$(grep -m 1 -E '^valid-after [0-9]{4}-[0-9]{2}-[0-9]{2} [0-9]{2}:[0-9]{2}:[0-9]{2}$' "$CONSENSUS" || true)
  [[ -n $line ]] && date -u -d "${line#valid-after } UTC" +%s
  return 0
}

record() {
  mkdir -p "$STATE_DIR"
  printf '%s %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$*" > "$LAST"
}

cmd_check() {
  [[ $EUID -eq 0 ]] || die "run as root"
  [[ -r $SOURCES ]] || die "no sources file: $SOURCES"
  command -v curl > /dev/null || die "curl is not installed"
  local onions=() line samples="" onion
  while read -r line; do
    line=${line%%#*}
    line=${line//[[:space:]]/}
    [[ $line =~ ^[a-z2-7]{56}\.onion$ ]] && onions+=("$line")
  done < "$SOURCES"
  (( ${#onions[@]} > 0 )) || die "no onion addresses in $SOURCES"

  # All sources at once: with Tor down each request waits for its timeout.
  mkdir -p "$STATE_DIR"
  local work i=0
  work=$(mktemp -d "$STATE_DIR/samples.XXXXXX")
  for onion in "${onions[@]}"; do
    sample "$onion" > "$work/$i" &
    i=$((i + 1))
  done
  wait
  samples=$(cat "$work"/* 2>/dev/null || true)
  rm -rf "$work"
  local answered
  answered=$(grep -c . <<< "$samples" || true)

  local decision action value
  decision=$(decide <<< "$samples")
  action=${decision%% *}
  value=${decision#* }
  case $action in
    ok)
      record "ok: off by $value s (no change), $answered of ${#onions[@]} sources answered"
      ;;
    step)
      local target floor
      target=$(( $(date +%s) + value ))
      floor=$(consensus_start)
      if [[ -n $floor ]] && (( target < floor - 3600 )); then
        record "WARNING: refused: the sources put the time more than an hour before Tor's consensus ($(date -u -d "@$floor" +%FT%TZ))"
        exit 3
      fi
      local before
      before=$(date +%s)
      date -u -s "@$target" > /dev/null
      if (( value < 0 )); then
        record "stepped by $value s, $answered of ${#onions[@]} sources answered. Until $(date -u -d "@$before" +%FT%TZ) a Tor restart (or remove-client) can make this host's onions unreachable until that time"
      else
        record "stepped by $value s, $answered of ${#onions[@]} sources answered"
      fi
      echo "clock stepped by $value s"
      ;;
    *)
      record "WARNING: clock not changed: $value ($answered of ${#onions[@]} sources answered)"
      # Too few answers or disagreement: worth another try. A limit: not.
      [[ $value == only* || $value == "sources disagree"* ]] && exit 1
      exit 3
      ;;
  esac
}

cmd_status() {
  if [[ -r $LAST ]]; then
    cat "$LAST"
  else
    echo "WARNING: no check since boot (timer: optes-clock.timer)"
  fi
}

case ${1:-} in
  check)
    [[ ${2:-} == --allow-back ]] && ALLOW_BACK=yes
    cmd_check ;;
  status) cmd_status ;;
  decide) decide ;;
  *) sed -n '2,33p' "$0"; exit 1 ;;
esac
