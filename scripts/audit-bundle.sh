#!/bin/bash
# Audit the size of a built AICC.app (and optionally its DMG).
#
# This script only reads. It never modifies the bundle it is pointed at: every
# strip projection is measured on a throw-away copy under $TMPDIR.
#
# Usage:
#   bash scripts/audit-bundle.sh                        # dist/mac/AICC.app
#   bash scripts/audit-bundle.sh --app PATH --out DIR
#   bash scripts/audit-bundle.sh --dmg dist/AICC-2.13.0.dmg
#   bash scripts/audit-bundle.sh --dmg-breakdown        # slower: rebuilds DMGs
#
# Outputs (in --out, default dist/audit):
#   audit.md    human-readable report
#   audit.csv   one row per measured component
#   audit.json  machine-readable summary
#
# Design notes:
#   * Sizes are reported twice: raw on disk, and (when --dmg is given) the DMG
#     is the only number that matters for what users download. A component that
#     looks large on disk but compresses to nothing is not a real target.
#   * "projected after strip -x" is measured, not estimated: we copy the binary
#     and run the real strip on it, then delete the copy.
#   * Anything we cannot measure is reported as `unavailable` instead of 0.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
APP="${AICC_APP:-$ROOT/dist/mac/AICC.app}"
DMG=""
OUT="$ROOT/dist/audit"
DMG_BREAKDOWN=0
KEEP_WORKDIR=0

usage() {
  cat <<'USAGE'
Usage: bash scripts/audit-bundle.sh [OPTIONS]

Optional:
  --app <path>        App bundle to audit (default: dist/mac/AICC.app)
  --dmg <path>        DMG to weigh; enables the compressed-size section
  --dmg-breakdown     Also measure each component's compressed contribution by
                      rebuilding throw-away DMGs (slow: ~1 min per component)
  --out <dir>         Output directory (default: dist/audit)
  --keep-workdir      Keep the temporary copies used for strip/DMG probes
  -h, --help          This message
USAGE
  exit 0
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --app)           APP="$2"; shift 2 ;;
    --dmg)           DMG="$2"; shift 2 ;;
    --dmg-breakdown) DMG_BREAKDOWN=1; shift ;;
    --out)           OUT="$2"; shift 2 ;;
    --keep-workdir)  KEEP_WORKDIR=1; shift ;;
    -h|--help)       usage ;;
    *) echo "Unknown argument: $1" >&2; usage ;;
  esac
done

if [[ ! -d "$APP" ]]; then
  echo "ERROR: app bundle not found: $APP" >&2
  echo "       build it first: bash macos/build-aicc-swiftui.sh" >&2
  exit 1
fi
if [[ -n "$DMG" && ! -f "$DMG" ]]; then
  echo "ERROR: dmg not found: $DMG" >&2
  exit 1
fi

mkdir -p "$OUT"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/aicc-audit.XXXXXX")"
cleanup() {
  if [[ "$KEEP_WORKDIR" == "1" ]]; then
    echo "workdir kept: $WORK"
  else
    rm -rf "$WORK"
  fi
}
trap cleanup EXIT

MD="$OUT/audit.md"
CSV="$OUT/audit.csv"
JSON="$OUT/audit.json"

# ---- helpers ----------------------------------------------------------------

kb() { du -sk "$1" 2>/dev/null | awk '{print $1}'; }
bytes_of() { stat -f%z "$1" 2>/dev/null || echo ""; }
mib() { awk -v k="$1" 'BEGIN { printf "%.2f", k / 1024 }'; }
sha() { shasum -a 256 "$1" 2>/dev/null | awk '{print $1}'; }

# csv_row <section> <component> <raw_kb> <strip_kb|-> <note>
CSV_ROWS=()
csv_row() {
  CSV_ROWS+=("$1,$2,$3,$4,$5")
}

plist() {
  /usr/libexec/PlistBuddy -c "Print :$1" "$APP/Contents/Info.plist" 2>/dev/null || echo "unknown"
}

APP_VERSION="$(plist CFBundleShortVersionString)"
APP_BUILD="$(plist CFBundleVersion)"
HOST_ARCH="$(uname -m)"
HOST_OS="$(sw_vers -productVersion 2>/dev/null || echo unknown)"

APP_KB="$(kb "$APP")"
APP_FILES="$(find "$APP" -type f 2>/dev/null | wc -l | tr -d ' ')"
APP_LINKS="$(find "$APP" -type l 2>/dev/null | wc -l | tr -d ' ')"

# Every Mach-O in the bundle, so nothing large hides in a subdirectory.
MACHO_LIST=()
while IFS= read -r -d '' candidate; do
  if file -b "$candidate" 2>/dev/null | grep -q 'Mach-O'; then
    MACHO_LIST+=("$candidate")
  fi
done < <(find "$APP" -type f -print0 2>/dev/null)

# strip projection: real strip on a copy, then measure the copy.
strip_projection_kb() {
  local src="$1" name
  name="$(basename "$src")"
  local copy="$WORK/strip-$RANDOM-$name"
  cp "$src" "$copy" 2>/dev/null || { echo ""; return; }
  xcrun strip -x "$copy" >/dev/null 2>&1 || { rm -f "$copy"; echo ""; return; }
  local out
  out=$(( $(bytes_of "$copy") / 1024 ))
  rm -f "$copy"
  echo "$out"
}

macho_segment_mb() {
  local file="$1" seg="$2"
  otool -l "$file" 2>/dev/null | awk -v want="$seg" '
    /segname/ { cur = $2 }
    /filesize/ && cur == want { sum += $2; cur = "" }
    END { if (sum > 0) printf "%.2f", sum / 1048576; else printf "-" }'
}

nsyms_of() {
  otool -l "$1" 2>/dev/null | awk '/LC_SYMTAB/ { found = 1 } found && /nsyms/ { print $2; exit }'
}

echo "Auditing $APP ..."

{
  echo "# AICC bundle audit"
  echo
  echo "| 项 | 值 |"
  echo "| --- | --- |"
  echo "| 采集时间 | $(date '+%Y-%m-%d %H:%M:%S %Z') |"
  echo "| 主机 | macOS $HOST_OS ($HOST_ARCH) |"
  echo "| App | \`$APP\` |"
  echo "| 版本 / build | $APP_VERSION / $APP_BUILD |"
  echo "| 体积（磁盘） | $(( APP_KB / 1024 )) MiB ($APP_KB KiB) |"
  echo "| 文件数 / 符号链接 | $APP_FILES / $APP_LINKS |"
  if [[ -n "$DMG" ]]; then
    echo "| DMG | \`$(basename "$DMG")\` |"
    echo "| DMG 体积 | $(bytes_of "$DMG") 字节 |"
    echo "| DMG SHA-256 | \`$(sha "$DMG")\` |"
  fi
  echo
} > "$MD"

# ---- 1. top-level components ------------------------------------------------

echo "==> 1. 顶层构成"
{
  echo "## 1. 顶层构成（磁盘体积）"
  echo
  echo "| 组件 | MiB | 占 App | 说明 |"
  echo "| --- | ---: | ---: | --- |"
} >> "$MD"

note_for() {
  case "$1" in
    Contents/Resources)        echo "资源根目录（其中 Python 运行时占绝大部分）" ;;
    Contents/Resources/Python) echo "内嵌 CPython 3.14.10 + 标准库（桥接运行时）" ;;
    Contents/Resources/Server) echo "桥接服务端 Python 源码（server.py 等）" ;;
    Contents/MacOS)            echo "Swift 主程序" ;;
    Contents/Helpers)          echo "Rust account-worker" ;;
    Contents/PlugIns)          echo "Widget 扩展" ;;
    Contents/_CodeSignature)   echo "签名数据" ;;
    Contents/Frameworks)       echo "空目录（仅为满足 bundle 布局）" ;;
    *)                         echo "" ;;
  esac
}

# Contents/* plus the biggest Resources/* subdirectories, so Python does not
# swallow the whole report.
while read -r size path; do
  rel="${path#"$APP"/}"
  pct=$(awk -v a="$size" -v b="$APP_KB" 'BEGIN { if (b > 0) printf "%.1f%%", a * 100 / b; else printf "-" }')
  printf '| `%s` | %s | %s | %s |\n' "$rel" "$(mib "$size")" "$pct" "$(note_for "$rel")" >> "$MD"
  csv_row "component" "$rel" "$size" "-" ""
done < <(du -sk "$APP/Contents"/* 2>/dev/null | sort -rn)

echo "" >> "$MD"
echo "Python 内部构成（磁盘体积）：" >> "$MD"
echo "" >> "$MD"
echo "| 路径 | MiB |" >> "$MD"
echo "| --- | ---: |" >> "$MD"
PY="$APP/Contents/Resources/Python"
if [[ -d "$PY" ]]; then
  while read -r size path; do
    rel="${path#"$PY"/}"
    printf '| `%s` | %s |\n' "$rel" "$(mib "$size")" >> "$MD"
    csv_row "python" "$rel" "$size" "-" ""
  done < <(du -sk "$PY"/* 2>/dev/null | sort -rn)
  echo "" >> "$MD"
  PY_PY=$(find "$PY" -name '*.py' 2>/dev/null | wc -l | tr -d ' ')
  PY_PYC=$(find "$PY" -name '*.pyc' 2>/dev/null | wc -l | tr -d ' ')
  PY_SO=$(find "$PY" \( -name '*.so' -o -name '*.dylib' \) 2>/dev/null | wc -l | tr -d ' ')
  PY_LIB_KB="$(kb "$PY/lib")"
  PY_BIN_KB="$(kb "$PY/bin")"
  PY_SITE_KB="$(kb "$PY/lib/python3.14/site-packages")"
  cat >> "$MD" <<EOF
| 统计 | 值 |
| --- | ---: |
| 解释器目录 \`bin/\` | $(mib "$PY_BIN_KB") MiB |
| 标准库目录 \`lib/\` | $(mib "$PY_LIB_KB") MiB |
| 其中 \`site-packages/\` | $(mib "$PY_SITE_KB") MiB |
| \`.py\` 文件数 | $PY_PY |
| \`.pyc\` 文件数 | $PY_PYC |
| \`.so\` / \`.dylib\` 数 | $PY_SO |

EOF
  csv_row "python" "bin" "$PY_BIN_KB" "-" "interpreter"
  csv_row "python" "lib" "$PY_LIB_KB" "-" "stdlib"
  csv_row "python" "lib/site-packages" "$PY_SITE_KB" "-" "pip/vendored"
else
  echo "_无内嵌 Python 运行时（开发构建）_" >> "$MD"
  echo "" >> "$MD"
fi

# ---- 2. Mach-O detail -------------------------------------------------------

echo "==> 2. Mach-O 明细"
{
  echo "## 2. Mach-O 明细"
  echo
  echo "| 二进制 | 文件 MiB | __TEXT | __DATA* | __LINKEDIT | 符号数 | strip -x 后 MiB | 可省 MiB |"
  echo "| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |"
} >> "$MD"

TOTAL_STRIP_SAVING_KB=0
for binary in "${MACHO_LIST[@]}"; do
  rel="${binary#"$APP"/}"
  size_kb=$(( $(bytes_of "$binary") / 1024 ))
  text="$(macho_segment_mb "$binary" __TEXT)"
  d1="$(macho_segment_mb "$binary" __DATA)"
  d2="$(macho_segment_mb "$binary" __DATA_CONST)"
  data=$(awk -v a="${d1/-/0}" -v b="${d2/-/0}" 'BEGIN { printf "%.2f", a + b }')
  le="$(macho_segment_mb "$binary" __LINKEDIT)"
  syms="$(nsyms_of "$binary")"
  [[ -z "$syms" ]] && syms="-"
  stripped_kb="$(strip_projection_kb "$binary")"
  if [[ -n "$stripped_kb" ]]; then
    saving=$(( size_kb - stripped_kb ))
    [[ "$saving" -lt 0 ]] && saving=0
    TOTAL_STRIP_SAVING_KB=$(( TOTAL_STRIP_SAVING_KB + saving ))
    printf '| `%s` | %s | %s | %s | %s | %s | %s | **%s** |\n' \
      "$rel" "$(mib "$size_kb")" "$text" "$data" "$le" "$syms" "$(mib "$stripped_kb")" "$(mib "$saving")" >> "$MD"
    csv_row "macho" "$rel" "$size_kb" "$stripped_kb" "syms=$syms"
  else
    printf '| `%s` | %s | %s | %s | %s | %s | unavailable | - |\n' \
      "$rel" "$(mib "$size_kb")" "$text" "$data" "$le" "$syms" >> "$MD"
    csv_row "macho" "$rel" "$size_kb" "-" "syms=$syms"
  fi
done
{
  echo
  echo "合计可省（strip -x）：**$(mib "$TOTAL_STRIP_SAVING_KB") MiB**（磁盘体积）"
  echo
  echo "\`strip -x\` 只删除本地符号表项，不动动态符号、不动 \`__TEXT\`，"
  echo "所以对 Swift 反射 / Python 内建模块查找没有影响；Apple 的 release 构建同样会 strip。"
  echo
} >> "$MD"

# ---- 3. resources -----------------------------------------------------------

echo "==> 3. 资源构成与重复"
RES_KB="$(kb "$APP/Contents/Resources")"
{
  echo "## 3. 资源构成"
  echo
  echo "按扩展名统计（整个 bundle，含 Python 标准库）："
  echo
  echo "| 扩展名 | MiB | 文件数 |"
  echo "| --- | ---: | ---: |"
} >> "$MD"
# Extension histogram. The interpreter is a hyphenated name with a dot in it
# (python3.14), so match it before the generic "last dot wins" rule.
while IFS=$'\t' read -r ext mbytes count; do
  [[ -z "$ext" ]] && continue
  printf '| `%s` | %s | %s |\n' "$ext" "$mbytes" "$count" >> "$MD"
done < <(find "$APP" -type f -exec stat -f "%z %N" {} + 2>/dev/null | awk '
  {
    size = $1
    name = $2
    sub(/.*\//, "", name)
    if (name ~ /^python[0-9]+\.[0-9]+$/) ext = "(python-interpreter)"
    else if (name ~ /\./) { ext = name; sub(/^.*\./, "", ext) }
    else ext = "(no-ext)"
    s[ext] += size
    c[ext]++
  }
  END { for (k in s) printf "%s\t%.2f\t%d\n", k, s[k] / 1048576, c[k] }' | sort -t$'\t' -k2,2 -rn)

DUP_LIST="$(find "$APP" -type f -size +64k -exec shasum -a 256 {} + 2>/dev/null \
  | sort | awk '{ c[$1]++; if (c[$1] == 2) dup[$1] = $2; files[$1] = files[$1] " " $3 }
      END { for (k in c) if (c[k] > 1) printf "%s\t%d\t%s\n", dup[k], c[k], files[k] }')"
{
  echo
  if [[ -z "$DUP_LIST" ]]; then
    echo "重复内容（sha256 相同、> 64 KB）：**无**。"
  else
    echo "重复内容（sha256 相同、> 64 KB）："
    echo
    echo "| 字节 | 份数 | 路径 |"
    echo "| ---: | ---: | --- |"
    while IFS=$'\t' read -r b n f; do
      echo "| $b | $n | \`$f\` |" >> "$MD"
    done <<< "$DUP_LIST"
  fi
  echo
  echo "Resources 合计：$(mib "$RES_KB") MiB。"
  echo
} >> "$MD"

# ---- 4. DMG ---------------------------------------------------------------

if [[ -n "$DMG" ]]; then
  echo "==> 4. DMG 压缩后构成"
  DMG_BYTES="$(bytes_of "$DMG")"
  DMG_KB=$(( DMG_BYTES / 1024 ))
  {
    echo "## 4. DMG 压缩后构成"
    echo
    echo "磁盘 $(mib "$APP_KB") MiB → DMG $(mib "$DMG_KB") MiB（$DMG_BYTES 字节，压缩率 $(awk -v a="$DMG_KB" -v b="$APP_KB" 'BEGIN { printf "%.1f%%", a * 100 / b }')）。"
    echo "用户实际下载的是 DMG，所以下面的数字才是瘦身的目标。"
    echo
  } >> "$MD"

  if [[ "$DMG_BREAKDOWN" == "1" ]]; then
    echo "  测量各组件边际压缩贡献（逐个重建临时 DMG）..."
    dmg_for_dir() {
      local label="$1" mutate="$2" stage="$WORK/dmg-$1"
      local out="$WORK/$1.dmg"
      rm -rf "$stage"; mkdir -p "$stage"
      cp -R "$APP" "$stage/AICC.app"
      ln -sfn /Applications "$stage/Applications"
      # shellcheck disable=SC2016  # $stage 是故意留到 bash -c 里展开的
      /bin/bash -c "$mutate" _ "$stage" >/dev/null 2>&1 || true
      hdiutil create -volname "AICC audit" -srcfolder "$stage" -ov -format UDZO \
        "$out" >/dev/null 2>&1 || { echo ""; return; }
      bytes_of "$out"
    }
    FULL_BYTES="$(dmg_for_dir full ':')"
    if [[ -z "$FULL_BYTES" ]]; then
      echo "_无法重建 DMG（hdiutil 失败），跳过压缩贡献测量。_" >> "$MD"
    else
      {
        echo "| 组件 | 移除后的 DMG | 边际压缩贡献 |"
        echo "| --- | ---: | ---: |"
      } >> "$MD"
      for spec in \
        "Python|rm -rf \"\$1/AICC.app/Contents/Resources/Python\"" \
        "Python 解释器|rm -f \"\$1/AICC.app/Contents/Resources/Python/bin/python3.14\"" \
        "Python 标准库|rm -rf \"\$1/AICC.app/Contents/Resources/Python/lib\"" \
        "Rust worker|rm -rf \"\$1/AICC.app/Contents/Helpers\"" \
        "Swift 主程序|rm -rf \"\$1/AICC.app/Contents/MacOS\"" \
        "Widget|rm -rf \"\$1/AICC.app/Contents/PlugIns\""; do
        label="${spec%%|*}"
        mutate="${spec#*|}"
        b="$(dmg_for_dir "$(echo "$label" | tr ' ' '_')" "$mutate")"
        if [[ -z "$b" ]]; then
          echo "| $label | unavailable | - |" >> "$MD"
        else
          delta_bytes=$(( FULL_BYTES - b ))
          printf '| %s | %d 字节 | **%d 字节**（%s MiB） |\n' "$label" "$b" "$delta_bytes" "$(mib $(( delta_bytes / 1024 )))" >> "$MD"
          csv_row "dmg-contribution" "$label" "-" "-" "$delta_bytes"
        fi
      done
      echo "" >> "$MD"
      echo "重建出的完整 DMG（用于对照）：$FULL_BYTES 字节；输入 DMG：$DMG_BYTES 字节。" >> "$MD"
      echo "" >> "$MD"
      echo "> **口径**：上表是**边际贡献**——把某个组件整体删掉后重新压缩得到的差值。" >> "$MD"
      echo "> 压缩有布局依赖，这些差值**不能相加**，也不等于该组件在 DMG 里的精确字节数；" >> "$MD"
      echo "> 它只回答「动这个组件值不值」。可加总的只有本脚本直接测到的 DMG 体积本身。" >> "$MD"
      echo "两者差异来自 hdiutil 的卷元数据与时间戳，属于正常抖动。" >> "$MD"
      echo "" >> "$MD"
    fi
  else
    {
      echo "_未测量压缩贡献（加 \`--dmg-breakdown\` 打开；会重建若干临时 DMG，约 1 分钟）。_" >> "$MD"
      echo
    } >> "$MD"
  fi
fi

# ---- 5. CSV / JSON --------------------------------------------------------

{
  echo "section,component,raw_kb,after_strip_kb,note"
  for row in "${CSV_ROWS[@]}"; do echo "$row"; done
} > "$CSV"

json_escape() { printf '%s' "$1" | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g'; }

{
  echo "{"
  printf '  "generated_at": "%s",\n' "$(date -u '+%Y-%m-%dT%H:%M:%SZ')"
  printf '  "host": {"os": "macOS %s", "arch": "%s"},\n' "$(json_escape "$HOST_OS")" "$(json_escape "$HOST_ARCH")"
  printf '  "app": {"path": "%s", "version": "%s", "build": "%s", "disk_kb": %d, "files": %d, "symlinks": %d},\n' \
    "$(json_escape "$APP")" "$(json_escape "$APP_VERSION")" "$(json_escape "$APP_BUILD")" "$APP_KB" "$APP_FILES" "$APP_LINKS"
  if [[ -n "$DMG" ]]; then
    printf '  "dmg": {"path": "%s", "bytes": %s, "sha256": "%s"},\n' \
      "$(json_escape "$DMG")" "$(bytes_of "$DMG")" "$(sha "$DMG")"
  else
    echo '  "dmg": null,'
  fi
  printf '  "strip_projection_total_kb": %d,\n' "$TOTAL_STRIP_SAVING_KB"
  printf '  "python_present": %s,\n' "$([[ -d "$PY" ]] && echo true || echo false)"
  echo '  "rows": ['
  first=1
  for row in "${CSV_ROWS[@]}"; do
    section="${row%%,*}"; rest="${row#*,}"
    component="${rest%%,*}"; rest="${rest#*,}"
    raw="${rest%%,*}"; rest="${rest#*,}"
    stripped="${rest%%,*}"; note="${rest#*,}"
    [[ "$first" == "1" ]] || echo ","
    first=0
    printf '    {"section": "%s", "component": "%s", "raw_kb": "%s", "after_strip_kb": "%s", "note": "%s"}' \
      "$(json_escape "$section")" "$(json_escape "$component")" "$(json_escape "$raw")" \
      "$(json_escape "$stripped")" "$(json_escape "$note")"
  done
  echo
  echo '  ]'
  echo "}"
} > "$JSON"

echo
echo "报告：$MD"
echo "CSV ：$CSV"
echo "JSON：$JSON"
echo
sed -n '/^## 2/,/^## 3/p' "$MD" | head -20
