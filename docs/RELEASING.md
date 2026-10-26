# 发布流程（macOS / GitHub）

一台机器配一次，之后每次发版只跑一条命令。

## 0. 完成标准

`main` 上有合并后的代码、`vX.Y.Z` tag 已推送、GitHub Release 带 `AICC-X.Y.Z.dmg` 与 `.sha256`、`updates/aicc-update.json` 指向它、main CI 全绿。

## 1. 一次性配置

### 1.1 SSH（给 git 用）

`git push` / `git fetch` 走 22 端口，不经过 HTTP 代理，是最稳的通道。

```bash
ssh-keygen -t ed25519 -C "aicc-mac@$(hostname -s)" -f ~/.ssh/id_ed25519_aicc -N ""
cat > ~/.ssh/config <<'EOF'
Host github.com
  HostName github.com
  User git
  IdentityFile ~/.ssh/id_ed25519_aicc
  IdentitiesOnly yes
  AddKeysToAgent yes
  UseKeychain yes
EOF
chmod 700 ~/.ssh && chmod 600 ~/.ssh/config
ssh-add --apple-use-keychain ~/.ssh/id_ed25519_aicc
cat ~/.ssh/id_ed25519_aicc.pub      # 贴到 https://github.com/settings/ssh/new
ssh -T git@github.com               # 看到 "Hi <login>!" 即可
```

远端必须是 SSH：

```bash
git remote set-url origin git@github.com:easonwong2026-del/AICC.git
```

### 1.2 Personal access token（给 API 用）

建 release、传附件、改清单都要 REST API，SSH 覆盖不到。

打开 https://github.com/settings/tokens/new?scopes=repo&description=aicc-release → 直接 Generate token（`repo` 权限即可）→ 存进钥匙串：

```zsh
read -rs T; printf 'protocol=https\nhost=github.com\nusername=easonwong2026-del\npassword=%s\n\n' "$T" | git credential-osxkeychain store; unset T; echo stored
```

验证：

```bash
curl -s -o /dev/null -w '%{http_code}\n' \
  -H "Authorization: Bearer $(printf 'protocol=https\nhost=github.com\n\n' | git credential get | sed -n 's/^password=//p')" \
  https://api.github.com/user        # 200
```

注意：**这台机器是 zsh**，`read -p` 是 bash 专有参数，会报 `read: -p: no coprocess`，用 `read -rs`。
token 通常设 7–30 天有效期；过期后 API 返回 401，重做 1.2 即可，SSH 不受影响。

## 2. 每次发版

```bash
cd ~/WorkBuddy/aicc/AICC
# 1) 版本号与说明
echo 2.13.0 > VERSION
$EDITOR docs/releases/2.13.0.md          # 按 docs/releases/2.11.0.md 的六节结构写

# 2) 构建产物（ad-hoc 签名，未公证）
bash macos/build-release-mac.sh          # 产出 dist/AICC-2.13.0.dmg

# 3) 提交并打 tag（构建号在 macos/MenuBarApp/Info.plist 里手动 +1）
git add -A && git commit -m "chore(release): 2.13.0"
git tag -a v2.13.0 -m "AICC 2.13.0 / macOS Build 23"
git push origin main --tags

# 4) 一条命令发版
bash scripts/release-publish.sh --dry-run    # 先看要做什么
bash scripts/release-publish.sh              # 建 release、传 dmg + sha256、更新清单并推 main
```

`release-publish.sh` 幂等：release 已存在就复用，附件已存在就跳过，清单没变化就不提交。中途失败直接重跑。

## 3. 排障

| 症状 | 原因 | 处理 |
| --- | --- | --- |
| `could not read Username for 'https://github.com'` | 没有 token 或存空了 | 重做 1.2，粘完再回车 |
| `read: -p: no coprocess` | 用了 bash 写法 | zsh 用 `read -rs T` |
| `401 Bad credentials` | token 贴错/过期 | 重新生成并覆盖钥匙串 |
| `403 Resource not accessible` | token 缺 `repo` 权限 | 重发 token，勾 `repo` |
| `CONNECT tunnel failed, response 502`，`github.com` 打不开 | WorkBuddy 沙箱代理只放行 `www.github.com` / `api.github.com` / `uploads.github.com`，**不含裸域** | 走 SSH 和 API，不要依赖打开 github.com 网页；OAuth 类连接器（含 `gh auth login`）会一直失败 |
| GitHub 连接器报 `code=10103 github token endpoint rate limited` | WorkBuddy 后端换 token 被限流 | 等冷却；连接器只支持读 release，建 release 仍要用本脚本 |
| CI 红 | — | 修完推上去，tag 需要重新指：`git push -f origin vX.Y.Z`（release 已发布则先删 Release 再重指） |

## 4. 需要人眼确认的部分

- 装完新版本要**重新添加桌面小组件**：ad-hoc 签名每次构建 cdhash 都变，系统把旧实例当另一个 App 丢掉。额度小组件的配置不受影响。
- `hdiutil verify dist/AICC-X.Y.Z.dmg` 必须 VALID；`swift test` 在本机跑不了（CLT 无 XCTest），只靠 CI。
