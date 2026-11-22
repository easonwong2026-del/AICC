#!/bin/bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
APP_NAME="AICC"
VERSION="$(tr -d '[:space:]' < "$ROOT/VERSION")"
DMG_NAME="$APP_NAME-$VERSION.dmg"
APP_DIR="$ROOT/dist/mac/$APP_NAME.app"
STAGING_DIR="$ROOT/dist/dmg"
RELEASE_DIR="${RELEASE_DIR:-$ROOT/dist}"
DMG_PATH="$RELEASE_DIR/$DMG_NAME"
TEMP_DMG="$ROOT/dist/$DMG_NAME.tmp.dmg"

# 透传签名策略：调用方给了 AICC_ALLOW_ADHOC / AICC_SIGNING_IDENTITY 就照做，
# 否则由 build-aicc-swiftui.sh 决定（默认要求本机固定证书）。
BUNDLE_SERVER=1 "$ROOT/macos/build-aicc-swiftui.sh" >/dev/null

rm -rf "$STAGING_DIR" "$TEMP_DMG" "$DMG_PATH"
mkdir -p "$STAGING_DIR" "$RELEASE_DIR"
cp -R "$APP_DIR" "$STAGING_DIR/"
ln -s /Applications "$STAGING_DIR/Applications"

# 说明里要写 Build 号，所以先取，不能等到 DMG 生成之后（那时 heredoc 已经执行过）。
BUILD_VERSION="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleVersion' "$APP_DIR/Contents/Info.plist" 2>/dev/null || echo unknown)"

# 安装引导：必须和实测支持的安装位置一致。
#
# `/Applications` 与 `~/Applications` 都实测可启动（同一份签名、同一份二进制），
# 所以快捷方式保持系统标准位置。首次启动真正的拦阻来自 Gatekeeper：这个构建
# 用本机证书自签名、没有公证，macOS 会把进程停在 dyld 之前等确认，而提示框
# 属于 CoreServicesUIAgent、只有「完成 / 移到废纸篓」两个按钮 —— 没有 Dock
# 图标的 AICC 看起来就像"点开没反应"。说明里必须写清这一步，否则用户只能
# 以为是安装路径不对。
cat > "$STAGING_DIR/安装说明.txt" <<EOF
AICC ${VERSION}（Build ${BUILD_VERSION}）安装说明
========================================

1. 安装位置
   把 AICC.app 拖到同一个窗口里的 Applications 文件夹（系统标准位置）。
   想装在个人目录也可以：访达按 ⇧⌘H 打开个人目录，自己建一个 Applications
   文件夹拖进去。两个位置都实测可用。

2. 首次打开被 macOS 拦住（重要）
   这个构建用本机证书自签名，没有通过 Apple 公证。如果双击后没有任何窗口
   出现，屏幕上有一条「未打开"AICC"…Apple 无法验证…」，那不是卡死：进程被
   Gatekeeper 停在启动之前，正在等确认。

   正确做法（Apple 官方流程）：
     打开「系统设置 → 隐私与安全性」，在页面底部找到关于 AICC 的提示，
     点「仍要打开」，再输入一次密码确认。
   提示框里只有「完成」和「移到废纸篓」属于正常现象，新版 macOS 不再提供
   「打开」按钮。确认过这一次之后不会再提示。

   ⚠️ 下面这条命令仅用于开发排障（知道自己在做什么再用），不是推荐安装步骤：
       xattr -dr com.apple.quarantine "/Applications/AICC.app"
   它会绕过系统的安全确认，普通用户请优先用上面的「仍要打开」。

3. 退出 AICC
   点菜单栏的 AICC 图标 → 「退出 AICC」。

4. 回滚（必须先把 AICC 完全停掉）
   只替换 .app 不还原数据库不是受支持的回滚方式：${VERSION} 会把本地数据库
   升级到更高的 schema 版本，旧版本可能读不了。

   步骤：
   a) 退出 AICC（菜单栏 → 退出 AICC），确认没有残留：
         pgrep -fl "AICC.app/Contents"
      列表为空再进行下一步 —— 数据库还在被 worker 或内置 Python 写的时候
      覆盖文件会得到损坏的库。

   b) 用备份还原 App 和数据库（路径换成你的实际备份）：
         BACKUP="\$HOME/Library/Application Support/AICC-install-backups/<备份目录>"
         APP="\$HOME/Applications/AICC.app"
         APP_DATA="\$HOME/Library/Application Support/AICC-Dashboard/accounts"
         rm -rf "\$APP"
         ditto "\$BACKUP/app/AICC.app" "\$APP"
         sqlite3 "\$BACKUP/db/accounts.sqlite" ".backup '\$APP_DATA/accounts.sqlite'"

   c) 删掉旧库遗留的 WAL / SHM（AICC 已确认退出后才可删，否则会丢最近的写入）：
         rm -f "\$APP_DATA/accounts.sqlite-wal" "\$APP_DATA/accounts.sqlite-shm"

   d) 确认无误后重新打开 AICC。还原前后可以用
         sqlite3 "<库路径>" "PRAGMA integrity_check;"
      检查数据库是否完好。
EOF

# 压缩格式：UDBZ（bzip2），不是 hdiutil 的默认 UDZO。
#
# 实测（同一份 App 内容，v2.13.0 / 20,067,290 字节的 UDZO 基线）：
#   UDZO 默认级别   20,066,915   ← 本机重建对照
#                                 （正式发布件 AICC-2.13.0.dmg 是 20,067,290，
#                                  两者差 375 字节，来自 hdiutil 卷元数据抖动）
#   UDZO zlib-level=9 17,296,107
#   ULFO（lzfse）    17,243,561
#   UDBZ（bzip2）    17,150,001   ← -14.5%，就是这一行
# 默认的 zlib 级别只把内容压到 42.6%，而同一份镜像 `gzip -9` 能到 26.6%：
# 差的不是内容，是参数。
#
# 为什么要先定格式、再改二进制：在 UDZO 默认级别下，只把 worker 换成
# opt-level=z 的版本（小 3.4 MB），DMG 反而**变大** 217 KB —— 压缩布局
# 与参数会吃掉二进制的收益。先固定 UDBZ，后面每一步的字节收益才能独立核算。
#
# 兼容性：UDIF 的 bzip2 自 macOS 10.4 起就支持，`hdiutil attach/verify` 均正常；
# 本项目自签分发、不做公证，不存在额外限制。代价只是打包时 CPU 略高。
hdiutil create \
  -volname "$APP_NAME $VERSION" \
  -srcfolder "$STAGING_DIR" \
  -ov \
  -format UDBZ \
  "$TEMP_DMG" >/dev/null
mv "$TEMP_DMG" "$DMG_PATH"

# === 符号与包的身份链 ===
# 这份 DMG 里的二进制，和 dist/symbols 里的 dSYM 必须是同一次构建出来的。
# 只靠版本号 + build 号是证明不了这件事的：同版本重新构建会得到不同的 UUID。
# 校验方式是把 DMG 真挂起来，读卷内二进制的 UUID 和 sha256，与清单逐项比。
SYMBOLS_DIR="$ROOT/dist/symbols/$VERSION-$BUILD_VERSION"
DMG_SHA="$(shasum -a 256 "$DMG_PATH" | awk '{ print $1 }')"

if [[ -f "$SYMBOLS_DIR/manifest.txt" ]]; then
  echo "dmg_sha256 $DMG_SHA" >> "$SYMBOLS_DIR/manifest.txt"
  # 共享 runner 上 hdiutil attach 偶尔会以 "Resource temporarily unavailable"
  # 失败（diskimages-helper 抢不到资源）。重试是安全的：挂载是只读的。
  ATTACHED_POINT=""
  attach_dmg() {
    local attempt
    for attempt in 1 2 3; do
      if hdiutil attach "$DMG_PATH" -nobrowse -readonly -mountpoint "$MOUNT_POINT" >/dev/null 2>&1; then
        ATTACHED_POINT="$MOUNT_POINT"
        return 0
      fi
      echo "  WARNING: hdiutil attach 第 ${attempt} 次失败，2 秒后重试" >&2
      sleep 2
    done
    # 最后一次不指定挂载点，让系统自己选（某些 runner 上 -mountpoint 会一直失败）
    local out
    if out="$(hdiutil attach "$DMG_PATH" -nobrowse -readonly 2>/dev/null)"; then
      ATTACHED_POINT="$(printf '%s\n' "$out" | grep -o '/Volumes/.*' | tail -1)"
      [[ -n "$ATTACHED_POINT" ]] && return 0
    fi
    return 1
  }

  MOUNT_POINT="$(mktemp -d "${TMPDIR:-/tmp}/aicc-dmg-verify.XXXXXX")"
  if attach_dmg; then
    if bash "$ROOT/scripts/verify-symbols.sh" \
         --app "$ATTACHED_POINT/$APP_NAME.app" \
         --symbols "$SYMBOLS_DIR" \
         --dmg "$DMG_PATH"; then
      VERIFY_OK=1
    else
      VERIFY_OK=0
    fi
    hdiutil detach "$ATTACHED_POINT" >/dev/null 2>&1 || true
  else
    echo "ERROR: 无法挂载刚生成的 DMG，符号与包的对应关系无法证明" >&2
    VERIFY_OK=0
  fi
  rmdir "$MOUNT_POINT" 2>/dev/null || true
  if [[ "$VERIFY_OK" != "1" ]]; then
    rm -f "$DMG_PATH"
    echo "ERROR: DMG 与符号归档对不上，产物已删除（见上面 verify-symbols 的输出）" >&2
    exit 1
  fi
  echo "  DMG sha256 ${DMG_SHA}（已写入 $SYMBOLS_DIR/manifest.txt）"
else
  echo "WARNING: 没有符号清单（AICC_STRIP=0 的调试构建？），跳过符号与包的对应校验" >&2
fi

rm -rf "$STAGING_DIR"

echo "$DMG_PATH"
