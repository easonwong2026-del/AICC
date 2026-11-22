import SwiftUI

/// 每日签到（WorkBuddy CN）。
///
/// 表格一屏回答：谁签了、谁没签、签了得多少、自动开没开、现在能不能补。
/// 所有说明都压在一行；「谁签的 / 为什么失败」在行内一句，机制进 help。
struct ConsoleCheckinSection: View {
    @ObservedObject var store: ConsoleStore
    let rows: [CheckinRowDisplay]
    @EnvironmentObject private var nav: ConsoleNavigation

    /// 「去登录」找不到目标账号时的提示。找不到就**不跳转**，
    /// 免得账号页把选择回落到列表第一项、让人以为那是目标账号。
    @State private var loginTargetError: String?

    private enum Column {
        static let status: CGFloat = 96
        static let reward: CGFloat = 56
        static let lastRun: CGFloat = 92
        static let auto: CGFloat = 56
        static let action: CGFloat = 88
        static let accountMin: CGFloat = 140
    }

    var body: some View {
        VStack(alignment: .leading, spacing: ConsoleTheme.groupSpacing) {
            ConsoleSectionHeader(
                title: "每日签到",
                detail: rows.isEmpty ? nil : TaskCenterSummary.checkin(rows)
            ) {
                // 常用操作放在标题行：960×640 下也不会被长列表挤到滚动区外。
                if pendingCount > 0 {
                    Button("签到未完成账号（\(pendingCount)）") {
                        Task { await store.runPendingCheckins() }
                    }
                    .buttonStyle(ConsolePrimaryButtonStyle())
                    .disabled(store.checkinBatching || store.checkinActing != nil)
                    .help("先查每个账号的状态，只给服务端确认未签到的那些提交")
                }
            }

            if let notice = store.checkinNotice {
                ConsoleInlineNotice(kind: .info, message: notice)
            }
            if let error = store.checkinError, !rows.isEmpty {
                ConsoleInlineNotice(kind: .error, message: error, actionTitle: "重试") {
                    Task { await store.refreshCheckinStatus() }
                }
            }
            if let error = store.checkinRecheckError {
                // 复查没成：行保持上一份已确认状态，原因在这里单独说明。
                ConsoleInlineNotice(kind: .error, message: error)
            }
            if let warning = loginTargetError {
                ConsoleInlineNotice(kind: .warning, message: warning)
            }

            content
            footer
        }
    }

    // MARK: - 表格

    @ViewBuilder
    private var content: some View {
        if rows.isEmpty {
            if store.checkinLoading {
                ConsoleLoadingRow(text: "正在读取签到状态")
            } else if let error = store.checkinError {
                ConsoleEmptyState(
                    symbol: "exclamationmark.triangle",
                    title: "没有读到签到状态",
                    message: error,
                    actionTitle: "重试"
                ) {
                    Task { await store.refreshCheckinStatus() }
                }
            } else {
                ConsoleEmptyState(
                    symbol: "calendar.badge.checkmark",
                    title: "还没有可签到的账号",
                    message: "WorkBuddy CN 账号添加后会出现在这里。",
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
            TaskCenterHeaderCell(title: "最近执行", width: Column.lastRun)
            TaskCenterHeaderCell(title: "自动签到", width: Column.auto)
            TaskCenterHeaderCell(title: "操作", width: Column.action)
        }
        .padding(.horizontal, TaskCenterColumn.inset)
        .padding(.vertical, 8)
    }

    private func tableRow(_ row: CheckinRowDisplay) -> some View {
        HStack(spacing: TaskCenterColumn.spacing) {
            VStack(alignment: .leading, spacing: 2) {
                Text(row.name)
                    .font(ConsoleType.rowTitle)
                    .foregroundStyle(ConsoleTheme.text)
                    .lineLimit(1)
                    .truncationMode(.middle)
                    .help(row.name)
                Text(row.detail)
                    .font(ConsoleType.micro)
                    .foregroundStyle(row.status == .failed ? ConsoleTheme.danger : ConsoleTheme.tertiaryText)
                    .lineLimit(1)
                    .help(row.detail)
            }
            .frame(minWidth: Column.accountMin, maxWidth: .infinity, alignment: .leading)

            TaskCenterStatusCell(status: row.status, help: row.help)
                .frame(width: Column.status, alignment: .leading)

            Text(row.reward)
                .font(ConsoleType.rowTitle)
                .monospacedDigit()
                .foregroundStyle(row.hasReward ? ConsoleTheme.success : ConsoleTheme.tertiaryText)
                .lineLimit(1)
                .frame(width: Column.reward, alignment: .leading)
                .help(row.hasReward ? "服务端确认的今日奖励" : "服务端没有返回奖励数额")

            Text(row.lastRunText)
                .font(ConsoleType.micro)
                .foregroundStyle(ConsoleTheme.tertiaryText)
                .monospacedDigit()
                .lineLimit(1)
                .minimumScaleFactor(0.85)
                .frame(width: Column.lastRun, alignment: .leading)
                .help(row.lastRunText == "—" ? "还没有今天的签到记录" : "最近一次签到：\(row.lastRunText)")

            Toggle("", isOn: Binding(
                get: { row.autoEnabled },
                set: { enabled in
                    guard let raw = store.checkin.row(row.accountId) else { return }
                    Task { await store.setCheckinEnabled(raw, enabled: enabled) }
                }
            ))
            .toggleStyle(.switch)
            .controlSize(.mini)
            .labelsHidden()
            .frame(width: Column.auto, alignment: .leading)
            .disabled(!row.canToggleAuto)
            // macOS 的 switch 在 disabled 时几乎不变色；显式降透明度，让「灰着但看起来能点」不再发生。
            .opacity(row.canToggleAuto ? 1 : 0.45)
            .help(toggleHelp(row))

            actionCell(row)
                .frame(width: Column.action, alignment: .leading)
        }
        .padding(.horizontal, TaskCenterColumn.inset)
        .padding(.vertical, 8)
        .frame(minHeight: ConsoleTheme.rowHeight)
    }

    /// 开关的 help 必须与禁用状态说同一件事：不能按钮灰着、提示却在教人怎么开。
    private func toggleHelp(_ row: CheckinRowDisplay) -> String {
        if let lock = row.autoLockReason { return lock }
        return row.autoEnabled
            ? "关闭后每天 09:00 的自动检查会跳过这个账号"
            : "开启后每天 09:00 自动检查一次；服务端已签到就跳过"
    }

    /// 操作列只有一个按钮，文案与动作都来自 `CheckinRowAction`：
    /// 已完成 / 待确认只给「复查」（只读），待签到才给「立即签到」，授权失效给「去登录」。
    @ViewBuilder
    private func actionCell(_ row: CheckinRowDisplay) -> some View {
        switch row.action {
        case .none:
            if row.isBusy {
                ProgressView().controlSize(.mini)
            } else {
                Text("—")
                    .font(ConsoleType.body)
                    .foregroundStyle(ConsoleTheme.tertiaryText)
            }
        case .login:
            Button(row.action.title) { openLoginTarget(row) }
                .buttonStyle(ConsoleSecondaryButtonStyle())
                .help(row.actionHelp)
        case .signIn, .retry, .recheck, .check:
            Button(row.action.title) { perform(row) }
                .buttonStyle(ConsoleSecondaryButtonStyle())
                .disabled(!row.canPerformAction)
                .help(row.actionHelp)
        }
    }

    /// 纯导航：定位到该签到账号在 AI 账号页里的详情。找不到就停在原地并说明原因，
    /// 不提交授权、不签到、不切换客户端。
    private func openLoginTarget(_ row: CheckinRowDisplay) {
        let decision = ConsoleCheckinLoginRoute.decision(
            accountId: row.accountId,
            displayName: row.name,
            accounts: store.accounts,
            accountsError: store.accountsError
        )
        switch decision {
        case .open(let filter, let selection):
            loginTargetError = nil
            nav.openAccounts(filter: filter, selection: selection)
        case .blocked(let reason):
            loginTargetError = reason
        }
    }

    /// 只读动作只调单账号状态查询，绝不经过 `runCheckin`。
    private func perform(_ row: CheckinRowDisplay) {
        guard let raw = store.checkin.row(row.accountId) else { return }
        switch row.action {
        case .signIn, .retry:
            Task { await store.runCheckin(raw) }
        case .recheck, .check:
            Task { await store.recheckCheckin(accountId: row.accountId) }
        case .login, .none:
            break
        }
    }

    // MARK: - 页脚

    private var pendingCount: Int { store.checkin.pendingRows.count }

    @ViewBuilder
    private var footer: some View {
        if !rows.isEmpty {
            HStack(spacing: 10) {
                if store.checkinBatching {
                    ProgressView().controlSize(.small)
                    Text("正在逐个签到…")
                        .font(ConsoleType.micro)
                        .foregroundStyle(ConsoleTheme.secondaryText)
                }

                Spacer(minLength: 8)

                Text(scheduleText)
                    .font(ConsoleType.micro)
                    .foregroundStyle(ConsoleTheme.tertiaryText)
                    .lineLimit(1)
                    .help(ownershipHelp)
            }
        }
    }

    /// 没有账号开自动时不要显示一个「下次检查」时间：那会让人以为它真的会跑。
    private var scheduleText: String {
        store.checkinAuto.enabledAccounts == 0
            ? "自动签到未开启"
            : "下次自动检查 \(store.checkinAuto.nextRunText())"
    }

    private var ownershipHelp: String {
        let last = store.checkinAuto.lastTickText.map { "；上次检查 \($0)" } ?? ""
        return "计划 \(store.checkinAuto.planTime)（\(store.checkinAuto.timezone)）\(last)。"
            + store.checkinAuto.ownershipNote
    }
}
