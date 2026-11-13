import SwiftUI

/// 签到任务页里的 **WorkBuddy 每日签到** 区域（PR #69）。
///
/// 这一块负责三件事，别的都不做：
/// 1. 显示每个账号今天签没签、拿了多少奖励（**以服务端为准**）；
/// 2. 单账号自动开关（默认关）；
/// 3. 用户点「立即签到」/「立即签到未完成账号」时如实执行并回报。
///
/// 不显示"可能""预计"这类猜测：不知道的奖励写破折号，说不清的状态写「状态未知」。
struct ConsoleCheckinSection: View {
    @ObservedObject var store: ConsoleStore

    private var status: CheckinFleetStatus { store.checkin }
    private var auto: CheckinAutoStatus { store.checkinAuto }

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            header
            summary
            card
            footer
        }
    }

    // MARK: - 头部

    private var header: some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            ConsoleSectionTitle(title: "WorkBuddy 每日签到", detail: headerDetail)
            Spacer(minLength: 8)
            if status.counts.unchecked > 0 {
                ConsoleBadge(text: "\(status.counts.unchecked) 个未签到", tint: ConsoleTheme.warning)
            }
            if failedCount > 0 {
                ConsoleBadge(text: "\(failedCount) 个需处理", tint: ConsoleTheme.danger)
            }
            Button {
                Task { await store.refreshCheckinStatus() }
            } label: {
                Image(systemName: "arrow.triangle.2.circlepath")
                    .font(.system(size: 11, weight: .medium))
            }
            .buttonStyle(ConsoleIconButtonStyle())
            .disabled(store.checkinLoading || store.checkinBatching)
            .help("向服务端逐个账号查一次今日状态（只读，不会签到）")
        }
    }

    private var headerDetail: String {
        if store.checkinLoading { return "查询中…" }
        if store.checkinBatching { return "执行中…" }
        return status.headline
    }

    private var failedCount: Int {
        status.rows.filter(\.state.isProblem).count
    }

    // MARK: - 概览

    private var summary: some View {
        HStack(spacing: 10) {
            Text(status.headline)
                .font(.system(size: 12, weight: .medium))
                .foregroundStyle(ConsoleTheme.text)
            Text(status.enabledNote)
                .font(.system(size: 10))
                .foregroundStyle(ConsoleTheme.tertiaryText)
            if let credit = status.creditNote {
                ConsoleBadge(text: credit, tint: ConsoleTheme.success)
            }
            if status.cached {
                Text("本地记录")
                    .font(.system(size: 9))
                    .foregroundStyle(ConsoleTheme.tertiaryText)
                    .help("这一份来自本地签到记录，点右上角刷新才会向服务端核对")
            }
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 2)
    }

    // MARK: - 账号表

    private var card: some View {
        ConsoleCard(padding: 0) {
            if status.rows.isEmpty {
                ConsoleEmptyState(
                    symbol: "calendar.badge.checkmark",
                    title: "还没有可签到的账号",
                    message: "先到「AI 账号」页添加或从 Magpie 导入 WorkBuddy 账号。"
                )
            } else {
                VStack(spacing: 0) {
                    columnHeader
                    Divider().overlay(ConsoleTheme.border)
                    ForEach(Array(status.sortedRows.enumerated()), id: \.element.id) { index, row in
                        accountRow(row)
                        if index < status.rows.count - 1 {
                            Divider().overlay(ConsoleTheme.border).padding(.leading, 16)
                        }
                    }
                }
            }
        }
    }

    private var columnHeader: some View {
        HStack(spacing: 10) {
            Text("账号")
                .frame(width: 150, alignment: .leading)
            Text("今日状态")
                .frame(width: 96, alignment: .leading)
            Text("奖励")
                .frame(width: 56, alignment: .trailing)
            Text("自动签到")
                .frame(width: 74, alignment: .center)
            Spacer(minLength: 0)
            Text("操作")
                .frame(width: 70, alignment: .trailing)
        }
        .font(.system(size: 10))
        .foregroundStyle(ConsoleTheme.tertiaryText)
        .padding(.horizontal, 16)
        .padding(.vertical, 8)
        .background(ConsoleTheme.cardSubtle)
    }

    private func accountRow(_ row: CheckinAccountRow) -> some View {
        let busy = store.checkinActing == row.accountId || row.running
        return HStack(spacing: 10) {
            VStack(alignment: .leading, spacing: 2) {
                Text(row.nickname.isEmpty ? "未命名账号" : row.nickname)
                    .font(.system(size: 12, weight: .medium))
                    .foregroundStyle(ConsoleTheme.text)
                    .lineLimit(1)
                Text(row.detailText)
                    .font(.system(size: 9))
                    .foregroundStyle(ConsoleTheme.tertiaryText)
                    .lineLimit(2)
                    .fixedSize(horizontal: false, vertical: true)
            }
            .frame(width: 150, alignment: .leading)

            stateBadge(row).frame(width: 96, alignment: .leading)

            Text(row.creditsText)
                .font(.system(size: 11, weight: .medium))
                .monospacedDigit()
                .foregroundStyle(row.credits == nil ? ConsoleTheme.tertiaryText : ConsoleTheme.success)
                .frame(width: 56, alignment: .trailing)

            Toggle(
                "",
                isOn: Binding(
                    get: { row.enabled },
                    set: { enabled in
                        Task { await store.setCheckinEnabled(row, enabled: enabled) }
                    }
                )
            )
            .toggleStyle(.switch)
            .controlSize(.mini)
            .labelsHidden()
            .frame(width: 74, alignment: .center)
            .disabled(store.checkinActing != nil || store.checkinBatching)
            .help(row.enabled ? "关闭后会自动轮次跳过这个账号" : "开启后每天 09:00 后自动检查一次")

            Spacer(minLength: 0)

            Button(busy ? "执行中" : "立即签到") {
                Task { await store.runCheckin(row) }
            }
            .buttonStyle(ConsoleSecondaryButtonStyle())
            .disabled(busy || store.checkinActing != nil || store.checkinBatching)
            .frame(width: 70, alignment: .trailing)
            .help(
                row.state.isChecked
                    ? "今天已签到：会先查服务端，不会重复提交"
                    : "先查服务端状态，确认未签到才会提交"
            )
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 10)
    }

    private func stateBadge(_ row: CheckinAccountRow) -> some View {
        let tint: Color = {
            if row.state.isChecked { return ConsoleTheme.success }
            if row.state.isProblem { return ConsoleTheme.danger }
            if row.state == .unchecked { return ConsoleTheme.warning }
            return ConsoleTheme.tertiaryText
        }()
        return ConsoleBadge(text: row.state.title, tint: tint)
    }

    // MARK: - 底部

    private var footer: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 8) {
                Button("刷新全部状态") {
                    Task { await store.refreshCheckinStatus() }
                }
                .buttonStyle(ConsoleSecondaryButtonStyle())
                .disabled(store.checkinLoading || store.checkinBatching)
                .help("只查状态，不签到")

                Button("立即签到未完成账号") {
                    Task { await store.runPendingCheckins() }
                }
                .buttonStyle(ConsoleSecondaryButtonStyle())
                .disabled(
                    store.checkinBatching
                        || store.checkinActing != nil
                        || status.pendingRows.isEmpty
                )
                .help(
                    status.pendingRows.isEmpty
                        ? "没有服务端确认未签到的账号"
                        : "先查每个账号的状态，只给未签到的那些提交签到"
                )

                Spacer(minLength: 0)

                Text("最近检查：\(auto.lastTickText ?? "—")")
                    .font(.system(size: 10))
                    .foregroundStyle(ConsoleTheme.tertiaryText)
                Text("下次自动检查：\(auto.nextRunText())")
                    .font(.system(size: 10))
                    .foregroundStyle(ConsoleTheme.tertiaryText)
            }

            if let notice = store.checkinNotice {
                Text(notice)
                    .font(.system(size: 10))
                    .foregroundStyle(ConsoleTheme.secondaryText)
                    .fixedSize(horizontal: false, vertical: true)
            }
            if let error = store.checkinError {
                Text(error)
                    .font(.system(size: 10))
                    .foregroundStyle(ConsoleTheme.danger)
                    .fixedSize(horizontal: false, vertical: true)
            }
            Text("\(auto.ownershipNote) 计划时间 \(auto.planTime)（\(auto.timezone)），账号之间错峰执行；手动签到不受该时间限制。")
                .font(.system(size: 9))
                .foregroundStyle(ConsoleTheme.tertiaryText)
                .fixedSize(horizontal: false, vertical: true)
        }
    }
}
