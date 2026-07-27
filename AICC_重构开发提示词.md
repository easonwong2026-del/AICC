# AICC macOS 菜单栏应用重构 Prompt

请在现有 **AICC macOS 菜单栏项目**基础上继续开发，不要新建项目，不要重写现有数据采集逻辑。

## 一、目标

将项目统一命名为 **AICC**，定位为紧凑型 AI 状态中心，用于展示和管理：

- Codex Weekly 额度
- WorkBuddy 积分
- DeepSeek 余额或用量
- OpenCodex 运行状态
- AICC 同步状态

点击菜单栏图标后，打开一个小型数据看板，不再使用传统的长菜单。

---

## 二、看板设计

使用 SwiftUI `MenuBarExtra` 的 `.window` 样式。

### 尺寸

- 宽度控制在 `340–360px`
- 高度约 `360–420px`
- 正常状态下不滚动
- 点击外部自动关闭
- 支持浅色和深色模式

### 信息结构

#### 顶部

- 标题：`AICC`
- 状态摘要：所有服务正常 / 有服务异常 / 正在刷新
- 右侧只保留：
  - 刷新按钮
  - 设置按钮

#### Codex 主卡片

显示：

- Weekly 剩余百分比
- 重置日期和时间
- 一条细进度条

Codex 是主信息，占据完整宽度。

#### WorkBuddy 与 DeepSeek

左右两张紧凑卡片：

- WorkBuddy：积分余额
- DeepSeek：余额、今日消费、Token 用量或请求次数中最重要的一项
- 没有真实数据时显示“暂无数据”，禁止伪造内容

#### 服务区域

只保留两个状态：

- OpenCodex
- E-ink 同步

OpenCodex 行显示：

- 状态点
- 运行状态文字
- 开关
- “打开 Codex”按钮

E-ink 只展示同步状态，不提供主界面启停按钮。

#### 底部

- 左侧：上次刷新时间
- 右侧：更多按钮

更多菜单只保留：

- 关于 AICC
- 退出 AICC

---

## 三、OpenCodex 集成

新建独立的 `OpenCodexController`，不要把命令执行逻辑写在 SwiftUI View 中。

至少提供：

```swift
detectExecutable()
refreshStatus()
ensure()
stop()
restart()
openCodex()
```

### 路径检测

macOS GUI App 的 PATH 与终端不同，需优先执行：

```bash
/bin/zsh -lc 'command -v ocx'
```

同时检查：

```text
~/.npm-global/bin/ocx
/opt/homebrew/bin/ocx
/usr/local/bin/ocx
```

检测成功后保存绝对路径。

### 启动

OpenCodex 开关打开时执行：

```bash
ocx ensure
```

禁止执行：

```bash
ocx service install
```

启动后检查：

```text
http://127.0.0.1:10100/healthz
```

健康检查成功后才显示“运行中”。

### 停止

开关关闭时执行：

```bash
ocx stop
```

开关状态必须来自真实服务状态，不能只读取本地 Boolean。

所有命令异步执行，不得阻塞主线程或弹出终端窗口。

---

## 四、Codex 启动联动

通过 `NSWorkspace` 监听 Codex Desktop 启动。

当设置项“随 Codex 启动 OpenCodex”开启时：

1. 检测 Codex Desktop 启动
2. 检查 OpenCodex 状态
3. 未运行时执行 `ocx ensure`

点击“打开 Codex”时：

1. 检查 OpenCodex
2. 未运行则执行 `ocx ensure`
3. 等待 healthz 正常
4. 打开 Codex Desktop

默认不在 Codex 退出后停止 OpenCodex。

---

## 五、设置页面

所有低频功能移入设置。

### 常规

- 登录时启动 AICC
- 菜单栏显示内容
- 自动刷新间隔
- 通知设置
- 主题设置

### 数据源

分别管理：

- Codex
- WorkBuddy
- DeepSeek

每个数据源显示：

- 连接状态
- 最近更新时间
- 手动刷新
- 配置入口

Token、Cookie、API Key 不得明文展示。

### OpenCodex

- 随 Codex 自动启动 OpenCodex
- Codex 退出后停止 OpenCodex
- 启动 Codex 前等待代理就绪
- OCX 路径
- 重新检测 OCX
- 服务地址
- 打开 Dashboard
- 重启 OpenCodex
- 运行诊断

### E-ink

- 同步状态
- 同步间隔
- 最近同步时间
- 立即同步
- 显示模板
- 设备配置

### 高级

- 查看日志
- 显示数据目录
- 重启内部数据服务
- 导出诊断信息
- 清理缓存

---

## 六、视觉规范

整体风格：

- 原生 macOS
- 紧凑、克制、清晰
- 轻微玻璃质感
- 不使用夸张渐变
- 不堆叠大量描边卡片
- 不加入无意义图表

优先使用：

- SwiftUI
- SF Symbols
- Material
- RoundedRectangle
- 原生 Toggle
- 自定义细进度条

可选使用：

- Pow：仅用于轻微状态动画
- Swift Charts：仅在已有历史数据时使用

建议：

- 外边距：14–16px
- 卡片间距：8–10px
- 圆角：10–12px
- 主数字：20–26pt
- 次级文字：10–11pt
- 动画时长：0.15–0.3 秒

---

## 七、需要移出主界面的功能

从当前菜单中移除：

- Start Service
- Stop Service
- Restart Service
- Open Settings
- Open Dashboard
- Enable Launch at Login
- Disable Launch at Login
- Open Logs
- Reveal Server Folder

这些功能统一放入设置页面。

---

## 八、改名要求

将用户可见名称统一改为 `AICC`，包括：

- App 名称
- 设置窗口标题
- 关于页面
- 通知标题
- Dashboard 标题
- 菜单栏辅助功能名称

不要贸然修改 Bundle Identifier、Keychain Service Name、UserDefaults 标识和数据目录。

如必须修改，先实现旧数据迁移，避免登录信息或设置丢失。

---

## 九、状态与错误处理

所有数据源支持：

- loading
- ready
- stale
- unavailable
- error

单个模块失败时：

- 保留最近一次成功数据
- 显示简短错误状态
- 提供重试
- 不让整个看板进入错误页面

日志中禁止记录：

- API Key
- Token
- Cookie
- ChatGPT 登录凭据
- `auth.json` 内容

---

## 十、验收标准

1. 项目用户可见名称统一为 AICC
2. 点击菜单栏图标后显示紧凑型 SwiftUI 看板
3. 面板宽度不超过 360px
4. 正常状态下无滚动条
5. 显示 Codex、WorkBuddy、DeepSeek 核心数据
6. OpenCodex 状态来自真实 healthz
7. 开关打开执行 `ocx ensure`
8. 开关关闭执行 `ocx stop`
9. 不执行 `ocx service install`
10. 所有命令不弹出终端窗口
11. 点击“打开 Codex”时先确保 OpenCodex 就绪
12. 从 Dock 或 Finder 启动 Codex 时可自动执行 `ocx ensure`
13. 不影响 Codex / ChatGPT 登录状态
14. E-ink 内部服务不再显示主界面启停按钮
15. 日志、目录、登录启动等功能全部迁移到设置
16. 深色和浅色模式均正常
17. 原有数据采集和 E-ink 同步功能保持正常
18. 不使用虚构数据

---

## 十一、执行流程

1. 检查现有项目结构
2. 找出现有菜单、数据模型、设置和服务逻辑
3. 给出简短改造计划
4. 创建 Git 提交或备份
5. 完成 AICC 改名
6. 实现紧凑看板
7. 接入现有三类数据
8. 实现 OpenCodexController
9. 实现 Codex 启动监听
10. 重构设置页面
11. 完成深浅色适配
12. 构建、测试并修复问题
13. 输出修改文件清单、测试结果和待确认项

不要只输出设计说明，请直接修改并验证现有项目。
