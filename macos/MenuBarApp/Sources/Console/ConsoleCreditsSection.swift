import SwiftUI

/// 「签到任务」页的 Qoder 段：每日 Credits 的查询、手动领取、自动开关。
///
/// 与 WorkBuddy 的签到不是一回事，所以**没有**塞进上面那张任务表：
///
/// | | WorkBuddy 签到 | Qoder Credits |
/// | --- | --- | --- |
/// | 结算 | 北京时间 09:00，错过可当天补 | 服务端活动窗口，窗口关了就没有 |
/// | 接口 | `/v2/checkin` | `/sash/api/v1/me/campaigns`（仅有 CN 证据） |
/// | 奖励 | 每日 credits | 活动发放的 Credits |
///
/// 三条界面纪律：
/// 1. 自动开关**默认关闭**，且未验证的版本（International）开关是禁用的；
/// 2. 只有真的"今天能领"时按钮才可点，避免点下去只得到一句 already；
/// 3. 历史记录展示真实的 outcome，包括失败——不隐藏失败。
struct ConsoleCreditsSection: View {
    @ObservedObject var store: ConsoleStore

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                ConsoleSectionTitle(
                    title: "Qoder 每日 Credits",
                    detail: store.qoderCreditsAuto.isEmpty ? nil : "\(store.qoderCreditsAuto.count) 个账号"
                )
                Spacer(minLength: 8)
                if store.qoderCreditsLoading {
                    ProgressView().controlSize(.small)
                }
                Button("刷新") {
                    Task { await store.refreshQoderCredits() }
                }
                .buttonStyle(ConsoleSecondaryButtonStyle())
                .disabled(store.qoderCreditsLoading)
            }

            if let error = store.qoderCreditsError {
                ConsoleCard {
                    Text(error)
                        .font(.system(size: 11))
                        .foregroundStyle(ConsoleTheme.warning)
                }
            } else if store.qoderCreditsAuto.isEmpty {
                ConsoleCard {
                    Text("还没有 Qoder 账号。先在「AI 账号」里用浏览器授权或 PAT 添加一个，"
                        + "再回来这里查看每日 Credits。")
                        .font(.system(size: 11))
                        .foregroundStyle(ConsoleTheme.secondaryText)
                        .fixedSize(horizontal: false, vertical: true)
                }
            } else {
                ForEach(store.qoderCreditsAuto) { item in
                    row(item)
                }
            }

            if let notice = store.qoderCreditsNotice {
                Text(notice)
                    .font(.system(size: 10))
                    .foregroundStyle(ConsoleTheme.secondaryText)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
        .task {
            if store.qoderCreditsAuto.isEmpty {
                await store.refreshQoderCredits(silent: true)
            }
        }
    }

    private func row(_ item: QoderCreditsAutoItem) -> some View {
        ConsoleCard {
            VStack(alignment: .leading, spacing: 8) {
                HStack(spacing: 8) {
                    Text(item.displayName)
                        .font(.system(size: 12, weight: .semibold))
                        .lineLimit(1)
                    Text(item.editionName)
                        .font(.system(size: 10))
                        .foregroundStyle(ConsoleTheme.tertiaryText)
                    Spacer(minLength: 8)
                    if !item.supported {
                        Text("未支持")
                            .font(.system(size: 10, weight: .medium))
                            .padding(.horizontal, 7)
                            .padding(.vertical, 3)
                            .background(ConsoleTheme.tertiaryText.opacity(0.12), in: Capsule())
                            .foregroundStyle(ConsoleTheme.secondaryText)
                    } else if item.autoEnabled {
                        Text("自动已开")
                            .font(.system(size: 10, weight: .medium))
                            .padding(.horizontal, 7)
                            .padding(.vertical, 3)
                            .background(ConsoleTheme.accentSoft, in: Capsule())
                            .foregroundStyle(ConsoleTheme.accent)
                    } else {
                        Text("自动关闭")
                            .font(.system(size: 10, weight: .medium))
                            .padding(.horizontal, 7)
                            .padding(.vertical, 3)
                            .background(ConsoleTheme.tertiaryText.opacity(0.12), in: Capsule())
                            .foregroundStyle(ConsoleTheme.secondaryText)
                    }
                }

                HStack(spacing: 8) {
                    Button("检查今日") {
                        Task { await store.checkQoderCredits(accountId: item.accountId) }
                    }
                    .buttonStyle(ConsoleSecondaryButtonStyle())
                    .disabled(store.qoderCreditsBusy == item.accountId || !item.supported)

                    Button("立即领取") {
                        Task { await store.claimQoderCredits(accountId: item.accountId) }
                    }
                    .buttonStyle(ConsolePrimaryButtonStyle())
                    .disabled(store.qoderCreditsBusy == item.accountId || !item.supported)
                    .help(item.supported
                        ? "只领取服务端当前标记为可领的活动；已经领过会返回「已领过」"
                        : "该版本没有已证实的 Credits 接口，不会发请求")

                    Toggle("自动领取", isOn: Binding(
                        get: { item.autoEnabled },
                        set: { value in
                            Task { await store.setQoderCreditsAuto(accountId: item.accountId, enabled: value) }
                        }
                    ))
                    .toggleStyle(.switch)
                    .controlSize(.mini)
                    .font(.system(size: 10))
                    // 未验证的版本打开开关只会制造"为什么没领到"的假问题。
                    .disabled(!item.supported || store.qoderCreditsBusy == item.accountId)

                    if store.qoderCreditsBusy == item.accountId {
                        ProgressView().controlSize(.mini)
                    }
                    Spacer(minLength: 0)
                }

                if let last = item.lastRun {
                    HStack(spacing: 6) {
                        Text(last.triggerLabel)
                            .font(.system(size: 10))
                            .foregroundStyle(ConsoleTheme.tertiaryText)
                        Text(last.outcomeLabel)
                            .font(.system(size: 10, weight: .medium))
                            .foregroundStyle(last.isSuccess ? ConsoleTheme.success : ConsoleTheme.warning)
                        if let credits = last.credits {
                            Text("+\(WorkerQuotaRow.format(credits)) Credits")
                                .font(.system(size: 10))
                                .foregroundStyle(ConsoleTheme.secondaryText)
                        }
                        Text(ConsoleQuotaView.localTimestamp(last.executedAt))
                            .font(.system(size: 10))
                            .foregroundStyle(ConsoleTheme.tertiaryText)
                        Spacer(minLength: 0)
                    }
                } else {
                    Text("还没有领取记录")
                        .font(.system(size: 10))
                        .foregroundStyle(ConsoleTheme.tertiaryText)
                }

                if let runs = store.qoderCreditsRuns[item.accountId], !runs.isEmpty {
                    VStack(alignment: .leading, spacing: 3) {
                        ForEach(runs.prefix(5)) { run in
                            HStack(spacing: 6) {
                                Text(run.triggerLabel)
                                    .font(.system(size: 10))
                                    .foregroundStyle(ConsoleTheme.tertiaryText)
                                    .frame(width: 28, alignment: .leading)
                                Text(run.outcomeLabel)
                                    .font(.system(size: 10))
                                    .foregroundStyle(run.isSuccess ? ConsoleTheme.success : ConsoleTheme.warning)
                                if let detail = run.errorDetail, !detail.isEmpty {
                                    Text(detail)
                                        .font(.system(size: 10))
                                        .foregroundStyle(ConsoleTheme.tertiaryText)
                                        .lineLimit(1)
                                        .truncationMode(.tail)
                                }
                                Spacer(minLength: 0)
                                Text(ConsoleQuotaView.localTimestamp(run.executedAt))
                                    .font(.system(size: 10, design: .monospaced))
                                    .foregroundStyle(ConsoleTheme.tertiaryText)
                            }
                        }
                    }
                    .padding(.top, 2)
                }
            }
        }
    }
}
