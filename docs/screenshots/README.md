# AICC 2.5.0 截图验收

截图由 `scripts/render-provider-screenshots.sh` 离屏渲染真实 SwiftUI 组件生成（2x Retina PNG）。

## 固定状态卡片

| 文件 | 内容 |
| --- | --- |
| `legacy-codex-card.png` | CodexCard：34–36pt 主数字、加粗进度条 |
| `legacy-workbuddy-card.png` | WorkBuddyCard：30pt 主数字 + 积分单位 |
| `legacy-deepseek-card.png` | DeepSeekCard：30pt 主数字 + CNY 单位 |

## 改动前对照（按 2.4.4 样式重建）

| 文件 | 内容 |
| --- | --- |
| `before-codex.png` | 26pt 内联 “92% left”、细进度条 |
| `before-workbuddy.png` | 20pt 小号积分、无单位分层 |
| `before-deepseek.png` | 20pt 小号余额、无单位分层 |

截图外圈深色区域是离屏渲染画布，非卡片内容。
