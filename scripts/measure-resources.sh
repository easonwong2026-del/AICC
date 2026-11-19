#!/bin/bash
# Measure the resource footprint of AICC as a whole process group.
#
# Covers everything the App spawns: the Swift host, the Rust account worker, the
# bundled Python bridge, and (when the system has one running) the Widget
# extension process. Per process it records RSS, macOS physical footprint, CPU,
# thread count and open file descriptors; per run it records startup timings and
# whether anything is left behind after quit.
#
# Usage:
#   bash scripts/measure-resources.sh --mode sample --label idle-30min --warmup 120
#   bash scripts/measure-resources.sh --mode launch --label cold-start
#   bash scripts/measure-resources.sh --mode exit   --label quit-check
#
# Outputs (in --out, default dist/audit/resources):
#   samples.csv          one row per process per sample
#   summary-<label>.md    human-readable report for this run
#   summary-<label>.json  machine-readable report for this run
#   baseline.csv  append-only: one row per run, for before/after comparison
#
# Honest accounting: the RSS of several processes cannot be added into "total
# physical memory" because pages are shared. The group sum is reported as an
# upper bound, and the same caveat applies to summed footprints. Anything this
# machine cannot measure (wakeups without root, widget presence) is reported as
# `unavailable` instead of guessed.
#
# ## Safety boundary
#
# `--mode sample` is read-only: it never quits, never launches, never writes to
# the App. It is the default.
#
# `--mode launch` / `--mode exit` do touch a running App, so they refuse to run
# without explicit consent:
#
#   --allow-lifecycle-actions   required. Acknowledges that the running AICC
#                               will be quit.
#   --allow-real-data           required by launch. Acknowledges that the App
#                               being started uses the **real** account
#                               database under ~/Library/Application Support,
#                               so it may migrate it and may trigger scheduled
#                               work (check-in, travel). There is no isolated
#                               mode: AICC has no data-directory option, and
#                               `open` cannot inject environment variables.
#   --allow-foreign-instance-quit
#                               opt-in only. Without it, a running instance at a
#                               *different* install path (e.g. the 2.12.2 in
#                               ~/Applications while measuring dist/mac) makes
#                               the run **refuse**, because quitting someone's
#                               daily-use build to measure another one is not a
#                               measuring decision.
#
# Quitting is fail-closed: if any instance of the same bundle id is still alive
# afterwards, the script exits without launching anything. It never replaces a
# running App with a different build behind your back.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
APP="${AICC_APP:-$ROOT/dist/mac/AICC.app}"
# Filled in from the bundle under test (see `bundle_id_of "$APP"`), so a
# mismatched --app cannot silently operate on a different product.
BUNDLE_ID=""
MODE="sample"
LABEL=""
WARMUP=0
SAMPLES=3
INTERVAL=5
OUT="$ROOT/dist/audit/resources"
SWIFT_PID=""
PYTHON_PID=""
SETTLE_TIMEOUT=60
QUIT_TIMEOUT=20
ALLOW_LIFECYCLE=0
ALLOW_REAL_DATA=0
ALLOW_FOREIGN_QUIT=0

usage() {
  cat <<'USAGE'
Usage: bash scripts/measure-resources.sh [OPTIONS]

Modes:
  --mode sample   (default) sample an already running AICC
  --mode launch   quit AICC, start it fresh, time the startup, then sample
  --mode exit     quit AICC and verify nothing is left behind

Optional:
  --label <name>        Name of this measurement (written into every row)
  --app <path>          App bundle (default: dist/mac/AICC.app)
  --warmup <seconds>    Sleep before the first sample (default: 0)
  --samples <n>         Number of samples (default: 3)
  --interval <seconds>  Gap between samples (default: 5)
  --settle-timeout <s>  launch: give up waiting for a quiet system after this
                        many seconds (default: 60)
  --swift-pid <pid>     Pin the Swift host PID instead of discovering it
  --python-pid <pid>    Pin the Python bridge PID instead of discovering it
  --out <dir>           Output directory (default: dist/audit/resources)

Lifecycle consent (launch / exit only):
  --allow-lifecycle-actions   Required. The running AICC will be quit.
  --allow-real-data           Required by launch. The App uses the real account
                              database and may migrate it / run scheduled work.
  --allow-foreign-instance-quit
                              Allow quitting an instance at a different install
                              path (default: refuse).
  -h, --help            This message

`--mode sample` is read-only and needs none of the above.
USAGE
  exit 0
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --mode)           MODE="$2"; shift 2 ;;
    --label)          LABEL="$2"; shift 2 ;;
    --app)            APP="$2"; shift 2 ;;
    --warmup)         WARMUP="$2"; shift 2 ;;
    --samples)        SAMPLES="$2"; shift 2 ;;
    --interval)       INTERVAL="$2"; shift 2 ;;
    --settle-timeout) SETTLE_TIMEOUT="$2"; shift 2 ;;
    --swift-pid)      SWIFT_PID="$2"; shift 2 ;;
    --python-pid)     PYTHON_PID="$2"; shift 2 ;;
    --out)            OUT="$2"; shift 2 ;;
    --allow-lifecycle-actions)      ALLOW_LIFECYCLE=1; shift ;;
    --allow-real-data)              ALLOW_REAL_DATA=1; shift ;;
    --allow-foreign-instance-quit)  ALLOW_FOREIGN_QUIT=1; shift ;;
    -h|--help)        usage ;;
    *) echo "Unknown argument: $1" >&2; usage ;;
  esac
done

case "$MODE" in
  sample|launch|exit) ;;
  *) echo "ERROR: --mode must be sample, launch or exit (got '$MODE')" >&2; exit 1 ;;
esac
[[ -z "$LABEL" ]] && LABEL="$MODE"
if ! [[ "$WARMUP" =~ ^[0-9]+$ ]] || ! [[ "$SAMPLES" =~ ^[1-9][0-9]*$ ]] || ! [[ "$INTERVAL" =~ ^[0-9]+$ ]]; then
  echo "ERROR: --warmup and --interval must be non-negative integers; --samples must be positive" >&2
  exit 1
fi
if [[ ! -d "$APP" ]]; then
  echo "ERROR: app bundle not found: $APP" >&2
  exit 1
fi

mkdir -p "$OUT"
CSV="$OUT/samples.csv"
LABEL_SLUG="$(echo "$LABEL" | tr ' /:' '___')"
MD="$OUT/summary-$LABEL_SLUG.md"
JSON="$OUT/summary-$LABEL_SLUG.json"
BASE="$OUT/baseline.csv"
[[ -f "$CSV" ]]  || echo "timestamp,label,mode,role,pid,rss_kb,footprint_bytes,pcpu,threads,fds" > "$CSV"
[[ -f "$BASE" ]] || echo "timestamp,label,mode,roles,group_rss_kb,group_footprint_bytes,sum_pcpu,sum_threads,sum_fds,startup_seconds" > "$BASE"

WORK="$(mktemp -d "${TMPDIR:-/tmp}/aicc-measure.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT

now_s() { python3 -c 'import time; print("%.3f" % time.time())'; }
iso_now() { date '+%Y-%m-%dT%H:%M:%S%z'; }

# ---- discovery --------------------------------------------------------------

# One `ps` snapshot per discovery call. Walking the tree from a single snapshot
# keeps the set self-consistent: two snapshots can disagree about a worker that
# appeared or exited in between.
ps_tree() {
  # AICC_MEASURE_PS_FILE is a test seam: the guard tests feed a synthetic
  # process table so they can assert "would this have quit / launched?" without
  # a real App anywhere near them.
  if [[ -n "${AICC_MEASURE_PS_FILE:-}" ]]; then
    cat "$AICC_MEASURE_PS_FILE"
    return 0
  fi
  # Returns non-zero when the process table cannot be read. Callers must treat
  # that as "unknown", never as "nothing is running": a sandboxed shell once
  # made every guard believe the App was gone, and the script went on to launch
  # a second instance on top of a live one.
  if [[ -n "${AICC_MEASURE_PS_BROKEN:-}" ]]; then
    return 1
  fi
  ps -axo pid=,ppid=,command= 2>/dev/null
}

# Guard everything that acts on processes: without visibility we cannot tell
# "no instance" from "cannot see", and both lifecycle modes would be blind.
require_process_visibility() {
  if ps_tree >/dev/null 2>&1; then return 0; fi
  cat >&2 <<'MSG'
ERROR: 读不到进程表（ps 被拒绝或不可用）。

  没有进程表就无法判断是否已有 AICC 在运行，也就无法保证不会出现
  双实例抢端口/数据库。脚本在这里停止，而不是把"看不到"当成"没有"。

  如果你在沙箱里运行，请在宿主机上重跑；或者用 --swift-pid 显式指定实例。
MSG
  exit 4
}

root_pid() {
  if [[ -n "$SWIFT_PID" ]]; then
    echo "$SWIFT_PID"
    return 0
  fi
  ps_tree | awk -v bin="$APP/Contents/MacOS/AICC" '
    {
      line = $0
      sub(/^[ ]*/, "", line)
      split(line, f, " ")
      if (f[3] == bin) { print f[1]; exit }
    }'
}

die() {
  local code="$1"; shift
  echo "ERROR: $*" >&2
  exit "$code"
}

# Every running process whose argv[0] looks like an AICC host binary, as
# "pid<TAB>bundle path". Discovered by executable path rather than by bundle id
# because we need to know *where* each instance lives before asking it anything.
running_hosts() {
  ps_tree | awk '
    {
      line = $0
      sub(/^[ ]*/, "", line)
      split(line, f, " ")
      cmd = f[3]
      if (cmd ~ /\/Contents\/MacOS\/AICC$/) {
        sub(/\/Contents\/MacOS\/AICC$/, "", cmd)
        print f[1] "\t" cmd
      }
    }'
}

bundle_id_of() {
  /usr/libexec/PlistBuddy -c "Print :CFBundleIdentifier" "$1/Contents/Info.plist" 2>/dev/null || echo ""
}

# All instances that belong to the bundle we are measuring.
instances_of_bundle() {
  local pid path
  running_hosts | while IFS=$'\t' read -r pid path; do
    [[ -z "$pid" || -z "$path" ]] && continue
    if [[ "$(bundle_id_of "$path")" == "$BUNDLE_ID" ]]; then
      printf '%s\t%s\n' "$pid" "$path"
    fi
  done
}

# Every descendant of $1, one pid per line.
group_pids() {
  ps_tree | awk -v root="$1" '
    {
      line = $0
      sub(/^[ ]*/, "", line)
      split(line, f, " ")
      pid = f[1]; ppid = f[2]
      parent[pid] = ppid
      order[NR] = pid
    }
    END {
      for (i = 1; i <= NR; i++) {
        p = order[i]
        cur = p
        hops = 0
        while (cur != "" && cur != "0" && hops < 16) {
          if (cur == root) { print p; break }
          cur = parent[cur]
          hops++
        }
      }
    }'
}

role_of() {
  local cmd
  cmd="$(ps -o command= -p "$1" 2>/dev/null || true)"
  case "$cmd" in
    *aicc-account-worker*) echo "worker" ;;
    *Resources/Python/bin/python3*) echo "python" ;;
    *AICCWidget.appex*) echo "widget" ;;
    *"Contents/MacOS/AICC"*) echo "swift" ;;
    *) echo "other" ;;
  esac
}

process_metric() {
  local pid="$1" what="$2"
  case "$what" in
    rss)       { ps -o rss= -p "$pid" 2>/dev/null | tr -d ' ' | head -1; } || true ;;
    pcpu)      { ps -o pcpu= -p "$pid" 2>/dev/null | tr -d ' ' | head -1; } || true ;;
    threads)   { ps -M "$pid" 2>/dev/null | awk 'NR > 1 { n++ } END { print n + 0 }'; } || true ;;
    fds)       { lsof -nP -p "$pid" 2>/dev/null | awk 'NR > 1 { n++ } END { print n + 0 }'; } || true ;;
    footprint)
      { footprint -p "$pid" -f bytes 2>/dev/null | awk '
        /Footprint:/ { for (i = 1; i <= NF; i++) if ($i == "Footprint:") { print $(i + 1); exit } }'; } || true ;;
    *)         echo "" ;;
  esac
}

magpie_running() {
  if ps_tree | grep -q "Magpie.app/Contents/MacOS/Magpie"; then echo "yes"; else echo "no"; fi
}

default_read() {
  defaults read "$BUNDLE_ID" "$1" 2>/dev/null || echo "unavailable"
}

# ---- lifecycle helpers ------------------------------------------------------

quit_app() {
  if [[ -n "${AICC_MEASURE_TEST_LOG:-}" ]]; then
    printf 'quit %s\n' "$BUNDLE_ID" >> "$AICC_MEASURE_TEST_LOG"
    if [[ "${AICC_MEASURE_QUIT_RESULT:-ok}" == "timeout" ]]; then
      return 1
    fi
    if [[ -n "${AICC_MEASURE_PS_FILE_AFTER_QUIT:-}" ]]; then
      AICC_MEASURE_PS_FILE="$AICC_MEASURE_PS_FILE_AFTER_QUIT"
    fi
    return 0
  fi
  osascript -e "tell application id \"$BUNDLE_ID\" to quit" >/dev/null 2>&1 || true
  local waited=0
  while [[ "$waited" -lt "$QUIT_TIMEOUT" ]]; do
    # Not just our --app: *any* instance of this bundle id must be gone, or a
    # second instance would race us for the port and the database.
    [[ -z "$(instances_of_bundle)" ]] && return 0
    sleep 1
    waited=$(( waited + 1 ))
  done
  return 1
}

launch_app() {
  if [[ -n "${AICC_MEASURE_TEST_LOG:-}" ]]; then
    printf 'open %s\n' "$APP" >> "$AICC_MEASURE_TEST_LOG"
    return 0
  fi
  open -n "$APP"
}

# Consent gate: everything that can quit or start an App goes through here.
require_lifecycle_consent() {
  [[ "$MODE" == "sample" ]] && return 0
  if [[ "$ALLOW_LIFECYCLE" != "1" ]]; then
    die 2 "--mode $MODE 会退出正在运行的 AICC。确认后加 --allow-lifecycle-actions；只观察用 --mode sample"
  fi
  if [[ "$MODE" == "launch" && "$ALLOW_REAL_DATA" != "1" ]]; then
    cat >&2 <<'MSG'
ERROR: --mode launch 会用**真实**账号库启动 App。

  App 的数据目录固定在 ~/Library/Application Support/AICC-Dashboard，
  没有数据目录参数，`open` 也无法注入环境变量，所以本脚本提供不了"隔离
  数据目录"的模式。启动后的新构建可能迁移该库、并触发签到/旅行等定时任务。

  确认接受后加 --allow-real-data；只想观察现有实例则用 --mode sample。
MSG
    exit 2
  fi
}

# A running instance at another install path is someone's daily build, not a
# measurement fixture. Refuse unless the caller says so explicitly.
refuse_foreign_instances() {
  local foreign="" pid path
  while IFS=$'\t' read -r pid path; do
    [[ -z "$pid" || -z "$path" ]] && continue
    [[ "$path" == "$APP" ]] && continue
    foreign="$foreign $pid:$path"
  done < <(instances_of_bundle)
  if [[ -z "$foreign" ]]; then return 0; fi
  if [[ "$ALLOW_FOREIGN_QUIT" == "1" ]]; then
    echo "WARNING: 另一个安装位置的 AICC 正在运行，按 --allow-foreign-instance-quit 一并退出：$foreign" >&2
    return 0
  fi
  cat >&2 <<MSG
ERROR: 检测到另一个安装位置的 AICC 正在运行：$foreign

  正在测量的是：$APP
  退出/替换别人的日常构建不该是测量的副作用。先手动退出它，或明确加
  --allow-foreign-instance-quit。
MSG
  exit 2
}

BUNDLE_ID="$(bundle_id_of "$APP")"
[[ -n "$BUNDLE_ID" ]] || die 1 "无法从 $APP 读出 CFBundleIdentifier"
require_process_visibility
require_lifecycle_consent

STARTUP_SECONDS="unavailable"
LAUNCH_T_SWIFT="unavailable"
LAUNCH_T_WORKER="unavailable"
LAUNCH_T_PYTHON="unavailable"

if [[ "$MODE" == "launch" ]]; then
  refuse_foreign_instances
  echo "==> 关闭已在运行的实例（冷启动要干净基线）"
  # Fail closed: a quit that did not take means a second instance would race
  # this one for the bridge port and the account database. Report, do not open.
  if ! quit_app; then
    die 3 "已有实例在 ${QUIT_TIMEOUT}s 内没有退出，放弃启动（绝不制造双实例）"
  fi
  if [[ -n "$(instances_of_bundle)" ]]; then
    die 3 "退出后仍检测到同 Bundle ID 实例：$(instances_of_bundle | tr '\n' ' ')"
  fi

  echo "==> 启动 $APP"
  t0="$(now_s)"
  launch_app || { echo "ERROR: 无法启动 App" >&2; exit 1; }

  # Startup peak: the highest group footprint seen while the App is coming up.
  # Steady-state sampling below would miss it entirely.
  PEAK_LAUNCH_FP=0
  PEAK_LAUNCH_RSS=0
  quiet=0

  deadline=$(( $(date +%s) + SETTLE_TIMEOUT ))
  while [[ "$(date +%s)" -lt "$deadline" ]]; do
    root="$(root_pid || true)"
    if [[ -n "$root" ]]; then
      if [[ "$LAUNCH_T_SWIFT" == "unavailable" ]]; then
        LAUNCH_T_SWIFT="$(awk -v a="$t0" -v b="$(now_s)" 'BEGIN { printf "%.2f", b - a }')"
      fi
      while IFS= read -r p; do
        [[ -z "$p" ]] && continue
        case "$(role_of "$p")" in
          worker) [[ "$LAUNCH_T_WORKER" == "unavailable" ]] && LAUNCH_T_WORKER="$(awk -v a="$t0" -v b="$(now_s)" 'BEGIN { printf "%.2f", b - a }')" ;;
          python) [[ "$LAUNCH_T_PYTHON" == "unavailable" ]] && LAUNCH_T_PYTHON="$(awk -v a="$t0" -v b="$(now_s)" 'BEGIN { printf "%.2f", b - a }')" ;;
        esac
        fp="$(process_metric "$p" footprint)"
        rss="$(process_metric "$p" rss)"
        if [[ -n "$fp" ]] && [[ "$fp" -gt "$PEAK_LAUNCH_FP" ]]; then PEAK_LAUNCH_FP="$fp"; fi
        if [[ -n "$rss" ]] && [[ "$rss" -gt "$PEAK_LAUNCH_RSS" ]]; then PEAK_LAUNCH_RSS="$rss"; fi
      done <<< "$(group_pids "$root")"
      cpu="$(process_metric "$root" pcpu)"
      # "Settled" needs all three: the worker exists, the host is quiet, and it
      # stayed quiet for a few consecutive reads. A single low reading right
      # after fork only means the process has not started working yet.
      if [[ "$LAUNCH_T_WORKER" != "unavailable" ]] && [[ -n "$cpu" ]] \
         && awk -v c="$cpu" 'BEGIN { exit !(c + 0 < 2.0) }'; then
        quiet=$(( quiet + 1 ))
        if [[ "$quiet" -ge 3 ]]; then
          STARTUP_SECONDS="$(awk -v a="$t0" -v b="$(now_s)" 'BEGIN { printf "%.2f", b - a }')"
          break
        fi
      else
        quiet=0
      fi
    fi
    sleep 1
  done
  if [[ "$STARTUP_SECONDS" == "unavailable" ]]; then
    echo "WARNING: ${SETTLE_TIMEOUT}s 内 CPU 没有降到 2% 以下，启动耗时记为 unavailable" >&2
  fi
  echo "    首个 Swift 进程 ${LAUNCH_T_SWIFT}s · worker ${LAUNCH_T_WORKER}s · python ${LAUNCH_T_PYTHON}s"
  echo "    CPU 稳定（<2%）${STARTUP_SECONDS}s"
fi

LEFTOVERS=""
if [[ "$MODE" == "exit" ]]; then
  refuse_foreign_instances
  root="$(root_pid || true)"
  if [[ -z "$root" ]]; then
    echo "ERROR: AICC 没有在运行，退出清理无从测量" >&2
    exit 1
  fi
  echo "==> 请求退出 AICC (pid $root)"
  before_pids="$(group_pids "$root" | tr '\n' ' ')"
  QUIT_OK=0
  if quit_app; then
    QUIT_OK=1
  else
    echo "==> 退出在 ${QUIT_TIMEOUT}s 内没有生效" >&2
  fi
  sleep 2
  for pid in $before_pids; do
    if kill -0 "$pid" 2>/dev/null; then
      LEFTOVERS="$LEFTOVERS $pid:$(role_of "$pid")"
    fi
  done
  # An orphaned bridge is reparented to launchd, so walking the child tree from
  # the host is not enough: also match anything still running out of the bundle.
  ORPHANS=""
  while IFS= read -r pid; do
    [[ -z "$pid" ]] && continue
    case " $before_pids " in
      *" $pid "*) ;;
      *) ORPHANS="$ORPHANS $pid:$(role_of "$pid")" ;;
    esac
  done < <(ps_tree | awk -v app="$APP/" '
    {
      line = $0
      sub(/^[ ]*/, "", line)
      split(line, f, " ")
      if (index(f[3], app) == 1) print f[1]
    }')

  if [[ -z "$LEFTOVERS" && -z "$ORPHANS" ]]; then
    echo "==> 退出后无残留进程"
  else
    echo "==> 残留进程:${LEFTOVERS:- 无子进程}"
    [[ -n "$ORPHANS" ]] && echo "==> bundle 内仍存活（含脱离父子关系的孤儿）:$ORPHANS"
    LEFTOVERS="$LEFTOVERS$ORPHANS"
  fi
fi

# ---- sampling ---------------------------------------------------------------

GROUP_RSS=0
GROUP_FP=0
SUM_CPU=0
SUM_THR=0
SUM_FD=0
ROWS=""
SEEN_ROLES=""

if [[ "$MODE" != "exit" ]]; then
  if [[ "$WARMUP" -gt 0 ]]; then
    echo "==> 预热 ${WARMUP}s"
    sleep "$WARMUP"
  fi

  root="$(root_pid || true)"
  if [[ -z "$root" ]]; then
    echo "ERROR: 找不到运行中的 AICC（可用 --swift-pid 指定）" >&2
    exit 1
  fi
  echo "==> AICC 主进程 pid=$root"

  for i in $(seq 1 "$SAMPLES"); do
    echo "--- Sample $i/$SAMPLES ---"
    pids_now="$(group_pids "$root")"
    if [[ -n "$PYTHON_PID" ]] && ! grep -qx "$PYTHON_PID" <<< "$pids_now"; then
      pids_now="$pids_now"$'\n'"$PYTHON_PID"
    fi
    # The widget extension is started by the system when a widget is on screen,
    # not by AICC, so it has to be found independently of the child tree.
    widget_now="$(ps_tree | awk '
      {
        line = $0
        sub(/^[ ]*/, "", line)
        split(line, f, " ")
        if (f[3] ~ /AICCWidget\.appex\/Contents\/MacOS\/AICCWidget$/) print f[1]
      }')"

    for pid in $pids_now $widget_now; do
      [[ -z "$pid" ]] && continue
      role="$(role_of "$pid")"
      # A widget pid found by the pattern is a widget by definition; keep the
      # role stable even if the command line was truncated by `ps`.
      if [[ " $widget_now " == *" $pid "* && "$MODE" != "exit" ]]; then role="widget"; fi
      rss="$(process_metric "$pid" rss)"
      fp="$(process_metric "$pid" footprint)"
      cpu="$(process_metric "$pid" pcpu)"
      thr="$(process_metric "$pid" threads)"
      fd="$(process_metric "$pid" fds)"
      [[ -z "$fp" ]] && fp="unavailable"
      echo "$(iso_now),$LABEL,$MODE,$role,$pid,${rss:-},${fp},${cpu:-},${thr:-},${fd:-}" >> "$CSV"
      printf '  %-6s pid=%-7s RSS=%-8s footprint=%-10s CPU=%-5s threads=%-3s fd=%s\n' \
        "$role" "$pid" "${rss:-?}KB" "$fp" "${cpu:-?}" "${thr:-?}" "${fd:-?}"
      if [[ -n "$rss" ]]; then echo "$rss" >> "$WORK/$role.rss"; fi
      if [[ "$fp" != "unavailable" ]]; then echo "$fp" >> "$WORK/$role.footprint"; fi
      if [[ -n "$cpu" ]]; then echo "$cpu" >> "$WORK/$role.pcpu"; fi
      if [[ -n "$thr" ]]; then echo "$thr" >> "$WORK/$role.threads"; fi
      if [[ -n "$fd" ]];  then echo "$fd"  >> "$WORK/$role.fds"; fi
      case " $SEEN_ROLES " in
        *" $role "*) ;;
        *) SEEN_ROLES="$SEEN_ROLES $role" ;;
      esac
    done

    if [[ "$i" -lt "$SAMPLES" ]]; then sleep "$INTERVAL"; fi
  done

  median_of() {
    [[ -f "$1" ]] || { echo "unavailable"; return; }
    sort -n "$1" | awk '{ a[NR] = $1 } END {
      if (NR == 0) { print "unavailable" }
      else if (NR % 2 == 1) { printf "%.0f\n", a[(NR + 1) / 2] }
      else { printf "%.0f\n", (a[NR / 2] + a[NR / 2 + 1]) / 2 }
    }'
  }
  peak_of() {
    [[ -f "$1" ]] || { echo "0"; return; }
    sort -n "$1" | tail -1
  }
  mb_of() {
    awk -v v="${1:-0}" 'BEGIN { if (v ~ /^[0-9]+$/) printf "%.1f", v / 1024; else printf "%s", v }'
  }

  for role in swift worker python widget other; do
    case " $SEEN_ROLES " in
      *" $role "*) ;;
      *) continue ;;
    esac
    m_rss="$(median_of "$WORK/$role.rss")"
    m_fp="$(median_of "$WORK/$role.footprint")"
    m_cpu="$(median_of "$WORK/$role.pcpu")"
    m_thr="$(median_of "$WORK/$role.threads")"
    m_fd="$(median_of "$WORK/$role.fds")"
    p_rss="$(peak_of "$WORK/$role.rss")"
    p_fp="$(peak_of "$WORK/$role.footprint")"
    [[ "$m_rss" != "unavailable" ]] && GROUP_RSS=$(( GROUP_RSS + m_rss ))
    [[ "$m_fp"  != "unavailable" ]] && GROUP_FP=$(( GROUP_FP + m_fp ))
    [[ "$m_cpu" != "unavailable" ]] && SUM_CPU=$(awk -v a="$SUM_CPU" -v b="$m_cpu" 'BEGIN { printf "%.1f", a + b }')
    [[ "$m_thr" != "unavailable" ]] && SUM_THR=$(( SUM_THR + m_thr ))
    [[ "$m_fd"  != "unavailable" ]] && SUM_FD=$(( SUM_FD + m_fd ))
    ROWS="$ROWS| $role | $m_rss | $m_fp | $m_cpu | $m_thr | $m_fd | $(mb_of "$p_rss") | $(awk -v v="$p_fp" 'BEGIN { printf "%.1f", v / 1048576 }') |"$'\n'
  done

  roles_csv="$(echo "$SEEN_ROLES" | tr ' ' '|' | sed 's/^|//')"
  echo "$(iso_now),$LABEL,$MODE,$roles_csv,$GROUP_RSS,$GROUP_FP,$SUM_CPU,$SUM_THR,$SUM_FD,$STARTUP_SECONDS" >> "$BASE"
fi
roles_csv="${roles_csv:-unknown}"

# ---- report -----------------------------------------------------------------

FP_MB="$(awk -v v="$GROUP_FP" 'BEGIN { printf "%.1f", v / 1048576 }')"

{
  echo "# AICC 资源测量：$LABEL"
  echo
  echo "| 项 | 值 |"
  echo "| --- | --- |"
  echo "| 采集时间 | $(iso_now) |"
  echo "| 模式 | $MODE |"
  echo "| App | \`$APP\` |"
  echo "| 版本 / build | $(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$APP/Contents/Info.plist" 2>/dev/null || echo unknown) / $(/usr/libexec/PlistBuddy -c 'Print :CFBundleVersion' "$APP/Contents/Info.plist" 2>/dev/null || echo unknown) |"
  echo "| 主机 | macOS $(sw_vers -productVersion 2>/dev/null) $(uname -m) |"
  echo "| 采样 | $SAMPLES 次 × ${INTERVAL}s（预热 ${WARMUP}s） |"
  echo "| Magpie 运行中 | $(magpie_running) |"
  echo "| 桥接开启 | $(default_read bridgeEnabled) |"
  echo "| 局域网桥接 | $(default_read bridgeLANEnabled) |"
  echo "| 旅行自动任务 | $(default_read 'aicc.travelAutoEnabled') |"
  echo "| 调试模式 | $(default_read debugMode) |"
  echo "| 小组件是否已添加 | unavailable（系统不提供可靠查询） |"
  echo "| 空闲唤醒次数 | unavailable（\`powermetrics\` 需要 root） |"
  echo
  if [[ "$STARTUP_SECONDS" != "unavailable" ]]; then
    echo "## 启动耗时"
    echo
    echo "| 里程碑 | 秒 |"
    echo "| --- | ---: |"
    echo "| Swift 主进程出现 | $LAUNCH_T_SWIFT |"
    echo "| Rust worker 出现 | $LAUNCH_T_WORKER |"
    echo "| Python 桥接出现 | $LAUNCH_T_PYTHON |"
    echo "| CPU 连续 3 次降到 2% 以下 | $STARTUP_SECONDS |"
    echo "| 启动期间峰值 footprint（单进程最大） | $PEAK_LAUNCH_FP 字节 |"
    echo "| 启动期间峰值 RSS（单进程最大） | $PEAK_LAUNCH_RSS KB |"
    echo
  fi
  if [[ "$MODE" == "exit" ]]; then
    echo "## 退出清理"
    echo
    if [[ "${QUIT_OK:-0}" == "1" ]]; then
      echo "| 退出请求 | 在 ${QUIT_TIMEOUT}s 内生效 |"
    else
      echo "| 退出请求 | **超时（${QUIT_TIMEOUT}s 内未退出）** |"
    fi
    if [[ -z "$LEFTOVERS" ]]; then
      echo "| 残留进程 | 无 |"
      echo
      echo "结论：**干净退出**。"
    else
      echo "| 残留进程 | \`$LEFTOVERS\` |"
      echo
      echo "结论：**失败** —— 退出后仍有进程存活，本次测量不计为成功。"
    fi
    echo
  else
    echo "## 各进程（中位数 / 峰值）"
    echo
    echo "| 角色 | RSS KB | footprint 字节 | CPU % | 线程 | FD | 峰值 RSS MB | 峰值 footprint MB |"
    echo "| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |"
    printf '%s' "$ROWS"
    echo
    echo "## 进程组合计"
    echo
    echo "| 指标 | 值 | 口径 |"
    echo "| --- | ---: | --- |"
    echo "| RSS 合计 | $(mb_of "$GROUP_RSS") MB | 上界：进程间共享页被重复计算 |"
    echo "| footprint 合计 | $FP_MB MB | 上界：同上 |"
    echo "| CPU 合计 | $SUM_CPU % | 各进程 \`ps %cpu\`（生命周期均值）之和 |"
    echo "| 线程合计 | $SUM_THR | |"
    echo "| FD 合计 | $SUM_FD | |"
    echo
  fi
  echo "## 原始数据"
  echo
  echo "- 逐样本：\`$CSV\`"
  echo "- 历史基线（每次运行一行）：\`$BASE\`"
  echo
  echo "> RSS 与 footprint 都是按进程统计的。多进程相加只能作为上界——dylib、"
  echo "> 框架页、shared cache 在各进程间共享，macOS 没有提供「整组独占物理内存」"
  echo "> 的直接读数。要精确值就以 \`footprint\` 的单进程数字为准。"
} > "$MD"

{
  echo "{"
  printf '  "generated_at": "%s",\n' "$(iso_now)"
  printf '  "label": "%s",\n' "$LABEL"
  printf '  "mode": "%s",\n' "$MODE"
  printf '  "app": "%s",\n' "$APP"
  printf '  "samples": %s,\n' "$SAMPLES"
  printf '  "interval_seconds": %s,\n' "$INTERVAL"
  printf '  "warmup_seconds": %s,\n' "$WARMUP"
  printf '  "startup_seconds": "%s",\n' "$STARTUP_SECONDS"
  printf '  "startup_peak": {"footprint_bytes": %s, "rss_kb": %s},\n' "${PEAK_LAUNCH_FP:-0}" "${PEAK_LAUNCH_RSS:-0}"
  printf '  "launch_milestones": {"swift": "%s", "worker": "%s", "python": "%s"},\n' \
    "$LAUNCH_T_SWIFT" "$LAUNCH_T_WORKER" "$LAUNCH_T_PYTHON"
  printf '  "context": {"magpie_running": "%s", "bridge_enabled": "%s", "bridge_lan": "%s", "travel_auto": "%s", "widget_present": "unavailable", "idle_wakeups": "unavailable"},\n' \
    "$(magpie_running)" "$(default_read bridgeEnabled)" "$(default_read bridgeLANEnabled)" "$(default_read 'aicc.travelAutoEnabled')"
  printf '  "group": {"roles": "%s", "rss_kb_upper_bound": %s, "footprint_bytes_upper_bound": %s, "pcpu_sum": %s, "threads_sum": %s, "fds_sum": %s},\n' \
    "$roles_csv" "${GROUP_RSS:-0}" "${GROUP_FP:-0}" "${SUM_CPU:-0}" "${SUM_THR:-0}" "${SUM_FD:-0}"
  printf '  "leftovers_after_quit": "%s",\n' "${LEFTOVERS:-}"
  echo '  "caveat": "RSS and footprint are per-process; summing across a process group double counts shared pages and is an upper bound only."'
  echo "}"
} > "$JSON"

echo
echo "报告：$MD"
echo "样本：$CSV"
echo "JSON：$JSON"
if [[ "$MODE" != "exit" ]]; then
  echo
  echo "进程组上界：RSS $(mb_of "$GROUP_RSS") MB · footprint $FP_MB MB · CPU $SUM_CPU %"
fi

# The exit run's whole result is its verdict: a report that says "residue" but
# exits 0 is the kind of green that hides a leak in CI. Report first, then fail.
if [[ "$MODE" == "exit" ]]; then
  if [[ "${QUIT_OK:-0}" != "1" ]]; then
    echo "FAIL: 退出请求在 ${QUIT_TIMEOUT}s 内没有生效（报告已写出：${MD}）" >&2
    exit 3
  fi
  if [[ -n "$LEFTOVERS" ]]; then
    echo "FAIL: 退出后仍有残留进程:${LEFTOVERS}（报告已写出：${MD}）" >&2
    exit 3
  fi
fi
