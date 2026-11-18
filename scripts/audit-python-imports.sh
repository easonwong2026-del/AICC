#!/bin/bash
# Report which standard-library files the bridge actually loads.
#
# The bridge ships a full CPython standard library (11 MiB of source).  Knowing
# what a real run touches is the only honest basis for pruning it: guessing by
# module name is how you ship an App that breaks the first time someone pairs a
# device at 3 a.m.
#
# How it measures: the interpreter is started with a `sitecustomize` that
# installs an audit hook on the `import` event and records the resolved file of
# every module in `sys.modules` at exit.  The real `server.py` runs for a few
# seconds, is interrupted with SIGINT, and the recorded set is diffed against
# the files on disk.
#
# Usage:
#   bash scripts/audit-python-imports.sh
#   bash scripts/audit-python-imports.sh --runtime dist/mac/AICC.app/Contents/Resources/Python
#   bash scripts/audit-python-imports.sh --seconds 10 --out dist/audit
#
# Outputs: python-imports.md, python-imports.csv
#
# Nothing here contacts the network, and the run is pointed at a throw-away data
# directory so it cannot disturb a real installation.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
RUNTIME="${AICC_RUNTIME:-$ROOT/dist/mac/AICC.app/Contents/Resources/Python}"
SERVER_DIR="${AICC_SERVER_DIR:-$ROOT/dist/mac/AICC.app/Contents/Resources/Server}"
SECONDS_TO_RUN=6
OUT="$ROOT/dist/audit"
PORT="${AICC_AUDIT_PORT:-18765}"

usage() {
  cat <<'USAGE'
Usage: bash scripts/audit-python-imports.sh [OPTIONS]

Optional:
  --runtime <dir>   Bundled runtime (default: dist/mac/AICC.app/Contents/Resources/Python)
  --server <dir>    Directory holding server.py (default: the bundled one)
  --seconds <n>     How long to let the bridge run (default: 6)
  --port <n>        Loopback port for the throw-away instance (default: 18765)
  --out <dir>       Output directory (default: dist/audit)
  -h, --help        This message
USAGE
  exit 0
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --runtime) RUNTIME="$2"; shift 2 ;;
    --server)  SERVER_DIR="$2"; shift 2 ;;
    --seconds) SECONDS_TO_RUN="$2"; shift 2 ;;
    --port)    PORT="$2"; shift 2 ;;
    --out)     OUT="$2"; shift 2 ;;
    -h|--help) usage ;;
    *) echo "Unknown argument: $1" >&2; usage ;;
  esac
done

PYTHON="$RUNTIME/bin/python3"
[[ -x "$PYTHON" ]] || { echo "ERROR: no interpreter at $PYTHON" >&2; exit 1; }
[[ -f "$SERVER_DIR/server.py" ]] || { echo "ERROR: no server.py in $SERVER_DIR" >&2; exit 1; }
mkdir -p "$OUT"

WORK="$(mktemp -d "${TMPDIR:-/tmp}/aicc-imports.XXXXXX")"
cleanup() { rm -rf "$WORK"; }
trap cleanup EXIT

HOOK_DIR="$WORK/hook"
mkdir -p "$HOOK_DIR"
cat > "$HOOK_DIR/sitecustomize.py" <<'PY'
"""Record every imported module and the file it came from."""
import atexit
import os
import sys

_loaded = set()


def _hook(event, args):
    if event == "import" and args and isinstance(args[0], str):
        _loaded.add(args[0])


def _dump():
    path = os.environ.get("AICC_IMPORT_LOG")
    if not path:
        return
    rows = []
    # Union of the audit hook's view and sys.modules: `encodings` and friends
    # are imported by the interpreter before sitecustomize runs, so the hook
    # alone would report them as untouched and prune a live dependency.
    for name in sorted(set(_loaded) | set(sys.modules)):
        module = sys.modules.get(name)
        origin = getattr(module, "__file__", None)
        if origin is None:
            continue  # builtin / frozen: no file on disk to prune
        rows.append("%s\t%s" % (name, os.path.realpath(origin)))
    with open(path, "w") as handle:
        handle.write("\n".join(rows) + "\n")


sys.addaudithook(_hook)
atexit.register(_dump)
PY

cat > "$WORK/drive.py" <<'PY'
"""Run server.py as __main__ and interrupt it after a few seconds.

A plain shell `kill -INT` from outside does not reliably reach the interpreter
(the shell wrapper sees its own process group), so the timer lives inside.
"""
import os
import runpy
import signal
import sys
import threading

target = os.environ["AICC_AUDIT_TARGET"]
delay = float(os.environ.get("AICC_AUDIT_SECONDS", "6"))

def _stop():
    os.kill(os.getpid(), signal.SIGINT)

threading.Timer(delay, _stop).start()
sys.argv = [target]
runpy.run_path(target, run_name="__main__")
PY

IMPORT_LOG="$WORK/imports.tsv"
DATA_DIR="$WORK/data"
mkdir -p "$DATA_DIR"

echo "==> 采集导入表（${SECONDS_TO_RUN}s，端口 ${PORT}，数据目录在临时目录内）"
set +e
EINK_PORT="$PORT" \
EINK_DATA_DIR="$DATA_DIR" \
AICC_IMPORT_LOG="$IMPORT_LOG" \
AICC_AUDIT_TARGET="$SERVER_DIR/server.py" \
AICC_AUDIT_SECONDS="$SECONDS_TO_RUN" \
PYTHONPATH="$HOOK_DIR" \
  "$PYTHON" -B "$WORK/drive.py" > "$WORK/bridge.log" 2>&1
BRIDGE_STATUS=$?
set -e

if [[ ! -s "$IMPORT_LOG" ]]; then
  echo "ERROR: 没有采到导入表，桥接可能启动失败。日志：" >&2
  sed -n '1,20p' "$WORK/bridge.log" >&2
  exit 1
fi

MODULE_COUNT=$(wc -l < "$IMPORT_LOG" | tr -d ' ')
echo "    记录到 $MODULE_COUNT 个有磁盘文件的模块（桥接退出码 ${BRIDGE_STATUS}）"

LIB_DIR="$RUNTIME/lib"
[[ -d "$LIB_DIR" ]] || { echo "ERROR: 找不到标准库目录 $LIB_DIR" >&2; exit 1; }

# Files under the runtime's lib that the run never loaded.
USED_LIST="$WORK/used.txt"
awk -F'\t' '{ print $2 }' "$IMPORT_LOG" | sort -u > "$USED_LIST"

TOTAL_FILES=$(find "$LIB_DIR" -name '*.py' | wc -l | tr -d ' ')
TOTAL_KB=$(du -sk "$LIB_DIR" | awk '{print $1}')

# Candidate = .py under lib that no loaded module resolved to. Bytecode and
# package data are counted separately so the numbers add up.
UNUSED_TSV="$WORK/unused.tsv"
: > "$UNUSED_TSV"
while IFS= read -r file; do
  real="$(python3 -c 'import os,sys; print(os.path.realpath(sys.argv[1]))' "$file")"
  if ! grep -qxF "$real" "$USED_LIST"; then
    size=$(stat -f%z "$file")
    printf '%s\t%d\n' "${file#"$LIB_DIR"/}" "$size" >> "$UNUSED_TSV"
  fi
done < <(find "$LIB_DIR" -name '*.py' | sort)

UNUSED_COUNT=$(wc -l < "$UNUSED_TSV" | tr -d ' ')
UNUSED_KB=$(awk -F'\t' '{ s += $2 } END { printf "%d", s / 1024 }' "$UNUSED_TSV")

CSV="$OUT/python-imports.csv"
MD="$OUT/python-imports.md"

{
  echo "module,file"
  awk -F'\t' '{ printf "%s,%s\n", $1, $2 }' "$IMPORT_LOG"
} > "$CSV"

# Per top-level directory summary of the not-loaded set.
BY_DIR="$WORK/by-dir.tsv"
awk -F'\t' '
  {
    path = $1
    size = $2
    sub(/^python[0-9.]+\//, "", path)
    if (path ~ /\//) { split(path, parts, "/"); key = parts[1] "/" }
    else { key = "(python3.x 顶层 .py)" }
    s[key] += size
    c[key]++
  }
  END { for (k in s) printf "%s\t%d\t%d\n", k, s[k], c[k] }
' "$UNUSED_TSV" | sort -t$'\t' -k2,2 -rn > "$BY_DIR"

mib() { awk -v b="$1" 'BEGIN { printf "%.2f", b / 1048576 }'; }

{
  echo "# 桥接实际导入的 Python 模块"
  echo
  echo "| 项 | 值 |"
  echo "| --- | --- |"
  echo "| 采集时间 | $(date '+%Y-%m-%d %H:%M:%S %Z') |"
  echo "| 运行时 | \`$RUNTIME\` |"
  echo "| 入口 | \`$SERVER_DIR/server.py\` |"
  echo "| 运行时长 | ${SECONDS_TO_RUN}s 后 SIGINT |"
  echo "| 有磁盘文件的模块 | $MODULE_COUNT |"
  echo "| 标准库 \`.py\` 总数 | $TOTAL_FILES |"
  echo "| 标准库目录体积 | $TOTAL_KB KiB |"
  echo "| 本次未加载的 \`.py\` | $UNUSED_COUNT |"
  echo "| 其中未被加载的字节数 | $UNUSED_KB KiB（$(mib $(( UNUSED_KB * 1024 ))) MiB） |"
  echo
  echo "## 未加载的 .py 按目录汇总"
  echo
  echo "| 目录 | KiB | 文件数 |"
  echo "| --- | ---: | ---: |"
  while IFS=$'\t' read -r dir bytes count; do
    printf '| `%s` | %s | %s |\n' "$dir" "$(( bytes / 1024 ))" "$count"
  done < "$BY_DIR"
  echo
  echo "## 已加载的模块（去重前 $MODULE_COUNT 条）"
  echo
  echo '```'
  awk -F'\t' '{ print $1 }' "$IMPORT_LOG" | sort -u | paste -sd ' ' - | fold -s -w 100
  echo '```'
  echo
  echo "> 这份清单只覆盖「跑起来会加载」的模块。运行时按需导入的路径（错误分支、"
  echo "> 罕见编码、证书校验失败分支）不会全部出现，所以**未加载 ≠ 可删除**："
  echo "> 删任何一个之前都要有对应的行为测试，而不是只看这张表。"
} > "$MD"

echo
echo "报告：$MD"
echo "清单：$CSV"
echo "未加载：$UNUSED_COUNT 个 .py，$UNUSED_KB KiB"
