#!/bin/bash
# 校验 AICC.app 里的账号中心助手：存在、可执行、是 Mach-O、签名有效、能自报版本。
# 用法：validate-account-worker-bundle.sh <AICC.app 路径>
set -euo pipefail

APP_DIR="${1:-dist/mac/AICC.app}"
HELPER="$APP_DIR/Contents/Helpers/aicc-account-worker"
WIDGET="$APP_DIR/Contents/PlugIns/AICCWidget.appex"

fail() { echo "FAIL: $*" >&2; exit 1; }

[[ -d "$APP_DIR" ]] || fail "找不到 App：$APP_DIR"
[[ -f "$HELPER" ]] || fail "缺少助手：$HELPER"
[[ -x "$HELPER" ]] || fail "助手不可执行：$HELPER"

file -b "$HELPER" | grep -q "Mach-O" || fail "助手不是 Mach-O 可执行文件：$(file -b "$HELPER")"

# 只允许出现在 Contents/Helpers/ 里，不能混进 Widget 扩展。
if [[ -d "$WIDGET" ]]; then
  if find "$WIDGET" -name "aicc-account-worker" | grep -q .; then
    fail "助手被错误地放进了 Widget 扩展，会破坏扩展签名"
  fi
fi

codesign --verify --strict "$HELPER" || fail "助手签名校验失败"
codesign --verify --strict "$APP_DIR" || fail "App 签名校验失败"

ARCHS="$(lipo -archs "$HELPER" 2>/dev/null || true)"
[[ -n "$ARCHS" ]] || fail "读不到助手架构"

VERSION_LINE="$("$HELPER" --version)"
echo "helper: $HELPER"
echo "arch:   $ARCHS"
echo "report: $VERSION_LINE"
echo "$VERSION_LINE" | grep -q "protocol" || fail "助手 --version 输出异常"

echo "OK: account worker bundle valid"
