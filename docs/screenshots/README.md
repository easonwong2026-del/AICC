# AICC 2.5.0 截图验收

截图由 `scripts/render-provider-screenshots.sh` 离屏渲染真实 SwiftUI 组件生成（2x Retina PNG）。

## 动态 Provider 卡片（P1 新设计）

| 文件 | 内容 |
| --- | --- |
| `provider-codex-light.png` | Codex 动态卡片：Weekly 92 大号主数字 + 5 Hour 次级 |
| `provider-workbuddy-light.png` | WorkBuddy 动态卡片：5,343.37 积分 大号主数字 |
| `provider-deepseek-light.png` | DeepSeek 动态卡片：128.50 CNY 余额 + 今日使用 |
| `provider-example-light.png` | 示例 Provider：零专属 SwiftUI 卡片自动展示 |
| `provider-long-number-light.png` | 长数字：999,999.99 / 1,234,567 正常显示 |
| `provider-unavailable-light.png` | Unavailable/Error：主区域显示“Temporarily unavailable”，不显示 0 |
| `provider-cached-light.png` | Cached：大号缓存积分 + 橙色 Cached + 缓存年龄 |
| `provider-workbuddy-dark.png` | 深色模式 |
| `provider-workbuddy-english.png` | 英文标签（Points Left / Used Today） |

## 当前 Dashboard 真实卡片（升级后）

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
