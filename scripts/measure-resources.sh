#!/bin/bash
# Measure RSS, CPU, threads, and file descriptors for AICC Swift and Python
# processes.
#
# Usage:
#   bash scripts/measure-resources.sh --label closed --samples 3
#   bash scripts/measure-resources.sh --label open --swift-pid 1234 --python-pid 5678
#
# This script does NOT start, stop, or replace the running AICC app or server.
# Panels must be opened or closed manually between measurement rounds.

set -euo pipefail

LABEL=""
WARMUP=120
SAMPLES=3
INTERVAL=5
SWIFT_PID=""
PYTHON_PID=""

usage() {
  cat <<'USAGE'
Usage: bash scripts/measure-resources.sh [OPTIONS]

Required:
  --label <closed|open>   Measurement context (e.g. panel closed / open)

Optional:
  --warmup <seconds>      Warm-up sleep before first sample (default: 120)
  --samples <n>           Number of successive samples (default: 3)
  --interval <seconds>    Gap between samples (default: 5)
  --swift-pid <pid>       Explicit AICC Swift process PID
  --python-pid <pid>      Explicit AICC Python server PID

If --swift-pid / --python-pid are omitted the script attempts to discover them
via pgrep. The user is responsible for ensuring only one AICC instance runs.
USAGE
  exit 0
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --label)     LABEL="$2";       shift 2 ;;
    --warmup)    WARMUP="$2";      shift 2 ;;
    --samples)   SAMPLES="$2";     shift 2 ;;
    --interval)  INTERVAL="$2";    shift 2 ;;
    --swift-pid) SWIFT_PID="$2";   shift 2 ;;
    --python-pid) PYTHON_PID="$2"; shift 2 ;;
    -h|--help)   usage ;;
    *) echo "Unknown argument: $1" >&2; usage ;;
  esac
done

if [[ -z "$LABEL" ]]; then
  echo "ERROR: --label is required (closed or open)" >&2
  exit 1
fi

if [[ "$LABEL" != "closed" && "$LABEL" != "open" ]]; then
  echo "ERROR: --label must be 'closed' or 'open', got '$LABEL'" >&2
  exit 1
fi

if ! [[ "$WARMUP" =~ ^[0-9]+$ ]] || ! [[ "$SAMPLES" =~ ^[1-9][0-9]*$ ]] || ! [[ "$INTERVAL" =~ ^[0-9]+$ ]]; then
  echo "ERROR: --warmup and --interval must be non-negative integers; --samples must be positive" >&2
  exit 1
fi

echo "=== AICC Resource Measurement ==="
echo "Label:    $LABEL"
echo "Warm-up:  ${WARMUP}s"
echo "Samples:  $SAMPLES"
echo "Interval: ${INTERVAL}s"
echo ""

# ---- Warm-up ----
if [[ "$WARMUP" -gt 0 ]]; then
  echo "Warming up for ${WARMUP}s..."
  sleep "$WARMUP"
fi

# ---- Discover PIDs ----
if [[ -z "$SWIFT_PID" ]]; then
  SWIFT_PID=$(pgrep -f "AICC$" 2>/dev/null | head -1 || echo "")
fi
if [[ -z "$PYTHON_PID" ]]; then
  PYTHON_PID=$(pgrep -f "python3.*server.py" 2>/dev/null | head -1 || echo "")
fi

if [[ -z "$SWIFT_PID" ]]; then
  echo "WARNING: AICC Swift process not found (use --swift-pid)" >&2
fi
if [[ -z "$PYTHON_PID" ]]; then
  echo "WARNING: AICC Python server not found (use --python-pid)" >&2
fi

# ---- Sampling ----
SWIFT_RSS_KB=()
SWIFT_CPU=()
SWIFT_THREADS=()
SWIFT_FDS=()
PYTHON_RSS_KB=()
PYTHON_CPU=()
PYTHON_THREADS=()
PYTHON_FDS=()
REQUESTS_UNAVAILABLE="unavailable (set AICC_ACCESS_LOG)"

for i in $(seq 1 "$SAMPLES"); do
  echo "--- Sample $i ---"

  if [[ -n "$SWIFT_PID" ]]; then
    ps_info=$(ps -o pid=,rss=,pcpu= -p "$SWIFT_PID" 2>/dev/null || true)
    if [[ -n "$ps_info" ]]; then
      read -r _ rss cpu <<< "$ps_info"
      threads=$(ps -M "$SWIFT_PID" 2>/dev/null | awk 'NR > 1 { count++ } END { print count + 0 }')
      SWIFT_RSS_KB+=("$rss")
      SWIFT_CPU+=("$cpu")
      SWIFT_THREADS+=("$threads")
      fds=$(lsof -nP -p "$SWIFT_PID" 2>/dev/null | awk 'NR > 1 { count++ } END { print count + 0 }')
      SWIFT_FDS+=("$fds")
      echo "  Swift PID=$SWIFT_PID RSS=${rss}KB CPU=${cpu}% threads=$threads FD=$fds"
    else
      echo "  Swift: process $SWIFT_PID not accessible"
    fi
  fi

  if [[ -n "$PYTHON_PID" ]]; then
    ps_info=$(ps -o pid=,rss=,pcpu= -p "$PYTHON_PID" 2>/dev/null || true)
    if [[ -n "$ps_info" ]]; then
      read -r _ rss cpu <<< "$ps_info"
      threads=$(ps -M "$PYTHON_PID" 2>/dev/null | awk 'NR > 1 { count++ } END { print count + 0 }')
      PYTHON_RSS_KB+=("$rss")
      PYTHON_CPU+=("$cpu")
      PYTHON_THREADS+=("$threads")
      fds=$(lsof -nP -p "$PYTHON_PID" 2>/dev/null | awk 'NR > 1 { count++ } END { print count + 0 }')
      PYTHON_FDS+=("$fds")
      echo "  Python PID=$PYTHON_PID RSS=${rss}KB CPU=${cpu}% threads=$threads FD=$fds"
    else
      echo "  Python: process $PYTHON_PID not accessible"
    fi
  fi

  # Request count: only if reliable access log available
  if [[ -n "${AICC_ACCESS_LOG:-}" ]] && [[ -f "$AICC_ACCESS_LOG" ]]; then
    count=$(grep -c . "$AICC_ACCESS_LOG" 2>/dev/null || echo 0)
    echo "  Requests (from $AICC_ACCESS_LOG): $count"
  else
    REQUESTS_UNAVAILABLE="unavailable (set AICC_ACCESS_LOG)"
    echo "  Requests: unavailable (set AICC_ACCESS_LOG to a readable server access log)"
  fi

  if [[ "$i" -lt "$SAMPLES" ]]; then
    sleep "$INTERVAL"
  fi
done

# ---- Median calculation ----
median() {
  local arr=("$@")
  if [[ ${#arr[@]} -eq 0 ]]; then echo "N/A"; return; fi
  IFS=$'\n' sorted=($(sort -n <<< "${arr[*]}")); unset IFS
  local mid=$(( ${#sorted[@]} / 2 ))
  if [[ $(( ${#sorted[@]} % 2 )) -eq 1 ]]; then
    echo "${sorted[$mid]}"
  else
    awk "BEGIN { printf \"%.0f\", (${sorted[$mid-1]} + ${sorted[$mid]}) / 2 }"
  fi
}

echo ""
echo "=== Results ($LABEL) ==="

if [[ ${#SWIFT_RSS_KB[@]} -gt 0 ]]; then
  echo "Swift RSS KB:      $(median "${SWIFT_RSS_KB[@]}")  (samples: ${SWIFT_RSS_KB[*]})"
  echo "Swift CPU %:       $(median "${SWIFT_CPU[@]}")  (samples: ${SWIFT_CPU[*]})"
  echo "Swift threads:     $(median "${SWIFT_THREADS[@]}")  (samples: ${SWIFT_THREADS[*]})"
  echo "Swift FDs:         $(median "${SWIFT_FDS[@]}")  (samples: ${SWIFT_FDS[*]})"
else
  echo "Swift: no data"
fi

if [[ ${#PYTHON_RSS_KB[@]} -gt 0 ]]; then
  echo "Python RSS KB:     $(median "${PYTHON_RSS_KB[@]}")  (samples: ${PYTHON_RSS_KB[*]})"
  echo "Python CPU %:      $(median "${PYTHON_CPU[@]}")  (samples: ${PYTHON_CPU[*]})"
  echo "Python threads:    $(median "${PYTHON_THREADS[@]}")  (samples: ${PYTHON_THREADS[*]})"
  echo "Python FDs:        $(median "${PYTHON_FDS[@]}")  (samples: ${PYTHON_FDS[*]})"
else
  echo "Python: no data"
fi

echo "Requests (period):  $REQUESTS_UNAVAILABLE"
echo ""
echo "NOTE: Panel was in '$LABEL' state. To measure the other state,"
echo "manually open or close the menu bar panel and re-run with the"
echo "opposite --label."
