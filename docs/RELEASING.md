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

# 2) 构建产物（默认用本机固定证书签名，未公证）
bash macos/build-release-mac.sh          # 产出 dist/AICC-2.13.0.dmg

# 3) 提交并打 tag（构建号在 macos/MenuBarApp/Info.plist 里手动 +1）
git add -A && git commit -m "chore(release): 2.13.0"
git tag -a v2.13.0 -m "AICC 2.13.0 / macOS Build 25"
git push origin main --tags

# 4) 一条命令发版
bash scripts/release-publish.sh --dry-run    # 先看要做什么
bash scripts/release-publish.sh              # 建 release、传 dmg + sha256、更新清单并推 main
```

`release-publish.sh` 幂等：release 已存在就复用，附件已存在就跳过，清单没变化就不提交。中途失败直接重跑。

**在打了 Tag 的那个提交上执行**：脚本要求本地 Tag 解析出的 commit 等于 `HEAD`，并且 `origin`
上同一个 Tag 指向同一 commit。在别的分支或更晚的提交上跑会直接拒绝。

三条安全约束（2026-11-18 复审后加固，`scripts/test-release-publish.sh` 覆盖）：

1. **`--dry-run` 不要求 Tag 存在。** 它的用途就是"打 Tag 之前先看计划"，
   现在只在实跑时强制本地与 `origin` 都有 Tag。
2. **上传失败立刻中止。** DMG 或 `.sha256` 的 HTTP 码不是 2xx 就直接 `die`，
   不碰 `updates/aicc-update.json`、不提交、不推送；上传完还会回查两个资产确实挂在
   release 上，再动清单。
3. **查询失败不许当成"不存在"**：Release 查询只有明确 404 才允许创建；资产列表查询必须
   200 且是合法 JSON。401/403/429/5xx/超时/坏 JSON 一律停下。
4. **Tag 必须指向正确提交**：本地 `vX.Y.Z` 解析出的 commit 必须等于 `HEAD`，`origin` 上的同一个
   Tag 也必须指向同一 commit。只"存在"不够。
5. **Mock 测试**：`bash scripts/test-release-publish.sh` 用假 `curl` + 本地 bare 仓库跑十种场景
   （dry-run 无 Tag、DMG/sha256 上传失败、实跑无 Tag、Release 查询 500 / 坏 JSON、
   资产列表 500、本地/远端 Tag 指向别处、正常路径；共 41 条断言），
   断言失败时没有任何副作用。CI 的 `swift-test` job 会跑它。

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

## 3.5 本机固定签名身份（为什么必须）

`macos/build-aicc-swiftui.sh` 现在**默认要求**一张固定的代码签名证书，找不到就报错，不再静默退回 ad-hoc。

原因：ad-hoc 签名的 designated requirement 是**这一次构建的 cdhash**。钥匙串条目的 ACL 记的是
「哪个程序可以读这条凭据」，所以每重建一次，系统就把新构建当成陌生程序，要用户重新输入登录密码。
固定证书的 DR 是：

```
designated => identifier "aicc-account-worker" and certificate root = H"4a60b0dd..."
```

它只绑定证书根，与某次构建无关 —— 这才是「授权一次、之后不再问」的前提。

| 场景 | 怎么构建 |
| --- | --- |
| 本机日常开发 / 验收 | 直接 `bash macos/build-aicc-swiftui.sh`（自动用 `AICC Local Code Signing`） |
| 换一张本机证书 | `AICC_LOCAL_IDENTITY="<证书名>"` 或 `AICC_SIGNING_IDENTITY="<证书名>"` |
| CI / 测速 / 临时验证 | `AICC_ALLOW_ADHOC=1`（会打印警告；产物**不可**用于安装验收） |
| 正式分发 | `AICC_SIGNING_IDENTITY="Developer ID Application: ..."` + 公证（只有这种身份才加 `--timestamp`） |

证书怎么来的：钥匙串访问 → 证书助理 → 创建证书 → 类型「代码签名」→ 名字 `AICC Local Code Signing`。
本机自签证书通常**没有**「信任设置」，所以 `security find-identity -p codesigning` 会显示
`0 valid identities`，但 `codesign` 依然可用（DR 只要求证书根）。构建脚本据此只看证书是否存在。

签名身份变更（换证书、或从 ad-hoc 切到证书）后，旧钥匙串条目里记的还是老身份，
**第一次**读凭据会再问一次密码，点「始终允许」即可；此后同一证书的构建不再询问。

Qoder 的原生条目（`Qoder CN App Safe Storage` / `Qoder App Safe Storage`）是第三方 ACL，
只信任 Qoder 客户端自己的签名（`teamid:B6U242QL73`），与 AICC 用什么证书无关 ——
这类授权继续按需在用户主动导入时触发，不纳入「AICC 自有凭据不弹窗」的验收标准。

## 4. 需要人眼确认的部分

- 装完新版本通常**不需要**重新添加桌面小组件：构建默认用固定证书签名，签名身份跨重建不变。
  只有显式 `AICC_ALLOW_ADHOC=1` 构建时，ad-hoc 的 cdhash 每次都变，系统会把旧实例当成另一个 App 丢掉，
  那种情况下要重新添加一次（额度小组件配置本身不受影响）。
- `hdiutil verify dist/AICC-X.Y.Z.dmg` 必须 VALID；`swift test` 在本机跑不了（CLT 无 XCTest），只靠 CI。
