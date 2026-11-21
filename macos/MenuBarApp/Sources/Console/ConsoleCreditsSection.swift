import SwiftUI

/// Qoder 每日 Credits（只有 Qoder CN 有已证实的活动接口）。
///
/// 这里只说「今天的活动奖励」：可领取 / 已领取 / 暂无活动 / 暂不支持 / 需要登录。
/// **不是账号余额**，所以页面上没有任何余额列，也不和额度相加。
struct ConsoleCreditsSection: View {
    @ObservedObject var store: ConsoleStore
    let rows: [CreditsRowDisplay]
    @EnvironmentObject private var nav: ConsoleNavigation

    @State private var runsAccountId: String?

    private enum Column {
        static let status: CGFloat = 92
        static let reward: CGFloat = 60
        static let auto: CGFloat = 56
        static let lastRun: CGFloat = 88
        static let action: CGFloat = 176
        static let accountMin: CGFloat = 140
    }

    var body: some View {
        VStack(alignment: .leading, spacing: ConsoleTheme.groupSpacing) {
            ConsoleSectionHeader(
                title: "每日 Credits",
                detail: TaskCenterSummary.credits(rows)
            ) {
                EmptyView()
            }

            if let notice = store.qoderCreditsNotice {
                ConsoleInlineNotice(kind: .info, message: notice)
            }
            if let error = store.qoderCreditsError, !rows.isEmpty {
                ConsoleInlineNotice(kind: .error, message: error, actionTitle: "重试") {
                    Task { await store.refreshQoderCredits() }
                }
            }

            content
            footer
        }
    }

    // MARK: - 表格

    @ViewBuilder
    private var content: some View {
        if rows.isEmpty {
            if store.qoderCreditsLoading {
                ConsoleLoadingRow(text: "正在读取每日 Credits")
            } else if let error = store.qoderCreditsError {
                ConsoleEmptyState(
                    symbol: "exclamationmark.triangle",
                    title: "没有读到 Credits 状态",
                    message: error,
                    actionTitle: "重试"
                ) {
                    Task { await store.refreshQoderCredits() }
                }
            } else {
                ConsoleEmptyState(
                    symbol: "circle.dotted.circle",
                    title: "还没有 Qoder 账号",
                    message: "Qoder CN 账号添加后会出现在这里；国际版没有已证实的接口。",
                    actionTitle: "打开 AI 账号"
                ) {
                    nav.page = .accounts
                }
            }
        } else {
            ConsoleListSurface(horizontalPadding: 0) {
                VStack(spacing: 0) {
                    header
                    Divider().overlay(ConsoleTheme.border)
                    ForEach(Array(rows.enumerated()), id: \.element.id) { index, row in
                        tableRow(row)
                        if index < rows.count - 1 {
                            Divider().overlay(ConsoleTheme.border)
                                .padding(.leading, TaskCenterColumn.inset)
                        }
                    }
                }
            }
        }
    }

    private var header: some View {
        HStack(spacing: TaskCenterColumn.spacing) {
            TaskCenterHeaderCell(title: "账号", width: nil)
            TaskCenterHeaderCell(title: "今日", width: Column.status)
            TaskCenterHeaderCell(title: "奖励", width: Column.reward)
            TaskCenterHeaderCell(title: "自动领取", width: Column.auto)
            TaskCenterHeaderCell(title: "最近执行", width: Column.lastRun)
            TaskCenterHeaderCell(title: "操作", width: Column.action)
        }
        .padding(.horizontal, TaskCenterColumn.inset)
        .padding(.vertical, 8)
    }

    private func tableRow(_ row: CreditsRowDisplay) -> some View {
        HStack(spacing: TaskCenterColumn.spacing) {
            VStack(alignment: .leading, spacing: 2) {
                Text(row.name)
                    .font(ConsoleType.rowTitle)
                    .foregroundStyle(ConsoleTheme.text)
                    .lineLimit(1)
                    .truncationMode(.middle)
                    .help(row.name)
                Text(row.edition)
                    .font(ConsoleType.micro)
                    .foregroundStyle(ConsoleTheme.tertiaryText)
                    .lineLimit(1)
            }
            .frame(minWidth: Column.accountMin, maxWidth: .infinity, alignment: .leading)

            TaskCenterStatusCell(status: row.status, help: row.detail)
                .frame(width: Column.status, alignment: .leading)

            VStack(alignment: .leading, spacing: 2) {
                Text(row.reward)
                    .font(ConsoleType.rowTitle)
                    .monospacedDigit()
                    .foregroundStyle(row.rewardNote == "待领取" ? ConsoleTheme.warning : ConsoleTheme.text)
                    .lineLimit(1)
                Text(row.rewardNote)
                    .font(ConsoleType.micro)
                    .foregroundStyle(ConsoleTheme.tertiaryText)
                    .lineLimit(1)
            }
            .frame(width: Column.reward, alignment: .leading)

            Toggle("", isOn: Binding(
                get: { row.autoEnabled },
                set: { enabled in
                    Task { await store.setQoderCreditsAuto(accountId: row.accountId, enabled: enabled) }
                }
            ))
            .toggleStyle(.switch)
            .controlSize(.mini)
            .labelsHidden()
            .frame(width: Column.auto, alignment: .leading)
            .disabled(!row.supportsAuto || row.isBusy)
            .help(row.supportsAuto
                  ? "每天按服务端活动窗口检查一次；不会增加额外领取"
                  : "该版本没有已证实的 Credits 接口")

            lastRunCell(row)
                .frame(width: Column.lastRun, alignment: .leading)

            actionCell(row)
                .frame(width: Column.action, alignment: .leading)
        }
        .padding(.horizontal, TaskCenterColumn.inset)
        .padding(.vertical, 8)
        .frame(minHeight: ConsoleTheme.rowHeight)
    }

    @ViewBuilder
    private func lastRunCell(_ row: CreditsRowDisplay) -> some View {
        if row.lastRunOutcome == "—" {
            Text("—")
                .font(ConsoleType.body)
                .foregroundStyle(ConsoleTheme.tertiaryText)
        } else {
            Button {
                runsAccountId = row.accountId
            } label: {
                VStack(alignment: .leading, spacing: 2) {
                    Text(row.lastRunOutcome)
                        .font(ConsoleType.micro)
                        .foregroundStyle(row.lastRunIsProblem ? ConsoleTheme.warning : ConsoleTheme.secondaryText)
                        .lineLimit(1)
                    Text(row.lastRunTime)
                        .font(ConsoleType.micro)
                        .foregroundStyle(ConsoleTheme.tertiaryText)
                        .lineLimit(1)
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .disabled(row.runs.isEmpty)
            .help(row.runs.isEmpty ? row.lastRunOutcome : "查看最近 \(row.runs.count) 次领取记录")
            .popover(isPresented: Binding(
                get: { runsAccountId == row.accountId },
                set: { if !$0 { runsAccountId = nil } }
            ), arrowEdge: .bottom) {
                runsPopover(row)
            }
        }
    }

    private func actionCell(_ row: CreditsRowDisplay) -> some View {
        HStack(spacing: 6) {
            Button("检查今日") {
                Task { await store.checkQoderCredits(accountId: row.accountId) }
            }
            .buttonStyle(ConsoleSecondaryButtonStyle())
            .disabled(!row.canCheck)
            .help(row.supportsAuto
                  ? "只读：向服务端确认今天有没有可领的活动"
                  : "该版本没有已证实的接口，不会发请求")

            // 只有本次会话确认「可领取」才可点：避免点下去只得到一句 already。
            Button("立即领取") {
                Task { await store.claimQoderCredits(accountId: row.accountId) }
            }
            .buttonStyle(ConsolePrimaryButtonStyle(enabled: row.canClaim))
            .disabled(!row.canClaim)
            .help(row.canClaim
                  ? "领取服务端标记为可领的活动；领完会回查"
                  : "先点「检查今日」，服务端确认可领后这里才会亮起")

            if row.isBusy {
                ProgressView().controlSize(.mini)
            }
        }
    }

    private func runsPopover(_ row: CreditsRowDisplay) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("领取记录")
                .font(ConsoleType.sectionTitle)
                .foregroundStyle(ConsoleTheme.text)
            if row.runs.isEmpty {
                Text("还没有记录")
                    .font(ConsoleType.small)
                    .foregroundStyle(ConsoleTheme.secondaryText)
            } else {
                ForEach(row.runs) { run in
                    HStack(alignment: .firstTextBaseline, spacing: 8) {
                        Text(run.trigger)
                            .font(ConsoleType.micro)
                            .foregroundStyle(ConsoleTheme.tertiaryText)
                            .frame(width: 28, alignment: .leading)
                        Text(run.outcome)
                            .font(ConsoleType.small)
                            .foregroundStyle(run.isProblem ? ConsoleTheme.warning : ConsoleTheme.secondaryText)
                        if !run.detail.isEmpty {
                            Text(run.detail)
                                .font(ConsoleType.micro)
                                .foregroundStyle(ConsoleTheme.tertiaryText)
                                .lineLimit(1)
                                .truncationMode(.tail)
                        }
                        Spacer(minLength: 8)
                        Text(run.time)
                            .font(ConsoleType.monoSmall)
                            .foregroundStyle(ConsoleTheme.tertiaryText)
                    }
                }
            }
        }
        .padding(14)
        .frame(width: 400)
    }

    // MARK: - 页脚

    @ViewBuilder
    private var footer: some View {
        if !rows.isEmpty {
            HStack(spacing: 6) {
                Text("每日 Credits 是活动奖励，不是账号余额，也不与额度相加。")
                    .font(ConsoleType.micro)
                    .foregroundStyle(ConsoleTheme.tertiaryText)
                    .lineLimit(1)
                    .help("活动窗口由服务端决定；窗口关闭后当天不能再领。此页只处理每日奖励，账号余额在额度看板。")
                Spacer(minLength: 0)
            }
        }
    }
}
