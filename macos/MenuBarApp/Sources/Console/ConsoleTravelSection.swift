import SwiftUI

/// Buddy 旅行（WorkBuddy CN）。
///
/// 一屏回答：现在在哪、到哪一步、能领多少、要不要派。
/// 页面加载只读本地快照；只有行内「领取 / 派出 / 刷新」会真的请求服务端。
struct ConsoleTravelSection: View {
    @ObservedObject var store: ConsoleStore
    let rows: [TravelRowDisplay]
    @EnvironmentObject private var nav: ConsoleNavigation

    private enum Column {
        static let statusMin: CGFloat = 120
        static let reward: CGFloat = 64
        static let auto: CGFloat = 56
        static let action: CGFloat = 158
        static let accountMin: CGFloat = 140
    }

    var body: some View {
        VStack(alignment: .leading, spacing: ConsoleTheme.groupSpacing) {
            ConsoleSectionHeader(
                title: "Buddy 旅行",
                detail: TaskCenterSummary.travel(rows)
            ) {
                EmptyView()
            }

            if let notice = store.travelNotice {
                ConsoleInlineNotice(kind: .info, message: notice)
            }
            if let error = store.travelError, !rows.isEmpty {
                ConsoleInlineNotice(kind: .error, message: error, actionTitle: "重试") {
                    Task { await store.loadTravel() }
                }
            }

            content
            footer
        }
        .task { await poll() }
    }

    // MARK: - 表格

    @ViewBuilder
    private var content: some View {
        if rows.isEmpty {
            if store.travelLoading {
                ConsoleLoadingRow(text: "正在读取旅行快照")
            } else if let error = store.travelError {
                ConsoleEmptyState(
                    symbol: "exclamationmark.triangle",
                    title: "读不到旅行状态",
                    message: error,
                    actionTitle: "重试"
                ) {
                    Task { await store.loadTravel() }
                }
            } else {
                ConsoleEmptyState(
                    symbol: "airplane",
                    title: "还没有 WorkBuddy 账号",
                    message: "账号添加后，Buddy 旅行会出现在这里。",
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
            TaskCenterHeaderCell(title: "当前状态", width: nil)
            TaskCenterHeaderCell(title: "奖励", width: Column.reward)
            TaskCenterHeaderCell(title: "自动", width: Column.auto)
            TaskCenterHeaderCell(title: "操作", width: Column.action)
        }
        .padding(.horizontal, TaskCenterColumn.inset)
        .padding(.vertical, 8)
    }

    private func tableRow(_ row: TravelRowDisplay) -> some View {
        HStack(spacing: TaskCenterColumn.spacing) {
            VStack(alignment: .leading, spacing: 2) {
                Text(row.name)
                    .font(ConsoleType.rowTitle)
                    .foregroundStyle(ConsoleTheme.text)
                    .lineLimit(1)
                    .truncationMode(.middle)
                    .help(row.name)
                Text(row.note)
                    .font(ConsoleType.micro)
                    .foregroundStyle(ConsoleTheme.tertiaryText)
                    .lineLimit(1)
            }
            .frame(minWidth: Column.accountMin, maxWidth: .infinity, alignment: .leading)

            VStack(alignment: .leading, spacing: 2) {
                Text(row.statusText)
                    .font(ConsoleType.body)
                    .foregroundStyle(statusColor(row.status))
                    .lineLimit(1)
                    .truncationMode(.tail)
                    .help(row.help.isEmpty ? row.statusText : "\(row.statusText)\n\(row.help)")
                if let warning = row.warning {
                    Text(warning)
                        .font(ConsoleType.micro)
                        .foregroundStyle(ConsoleTheme.warning)
                        .lineLimit(1)
                        .help(warning)
                }
            }
            .frame(minWidth: Column.statusMin, maxWidth: .infinity, alignment: .leading)

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
                    guard let raw = store.travelRows.first(where: { $0.accountId == row.accountId }) else { return }
                    Task { await store.setTravelEnabled(raw, enabled: enabled) }
                }
            ))
            .toggleStyle(.switch)
            .controlSize(.mini)
            .labelsHidden()
            .frame(width: Column.auto, alignment: .leading)
            .disabled(!row.canToggleAuto)
            // macOS 的 switch 在 disabled 时几乎不变色；显式降透明度，避免看起来还能点。
            .opacity(row.canToggleAuto ? 1 : 0.45)
            .help(toggleHelp(row))

            actionCell(row)
                .frame(width: Column.action, alignment: .leading)
        }
        .padding(.horizontal, TaskCenterColumn.inset)
        .padding(.vertical, 8)
        .frame(minHeight: ConsoleTheme.rowHeight)
    }

    /// 开关的 help 必须与禁用状态说同一件事。
    private func toggleHelp(_ row: TravelRowDisplay) -> String {
        if let lock = row.autoLockReason { return lock }
        return row.autoEnabled
            ? "关闭后不再自动派出与领取"
            : "开启后由 AICC 常驻助手自动派出并领取，仅在 AICC 运行时生效"
    }

    @ViewBuilder
    private func actionCell(_ row: TravelRowDisplay) -> some View {
        HStack(spacing: 6) {
            if row.isClaimable {
                Button("领取") {
                    run(.claim, row)
                }
                .buttonStyle(ConsolePrimaryButtonStyle(enabled: row.canClaim))
                .disabled(!row.canClaim)
                .help("领取到达奖励；服务端还会再校验一次，不会重复领取")
            }
            if row.isDispatchable {
                Button("派出") {
                    run(.depart, row)
                }
                .buttonStyle(ConsoleSecondaryButtonStyle())
                .disabled(!row.canDepart)
                .help("派出 Buddy；服务端的每日上限仍然生效")
            }
            Button {
                run(.refresh, row)
            } label: {
                Image(systemName: "arrow.triangle.2.circlepath")
                    .font(.system(size: 11, weight: .medium))
            }
            .buttonStyle(ConsoleIconButtonStyle(size: 26))
            .disabled(row.isBusy)
            .help("向服务端查一次真实状态（只读）")

            if row.isBusy {
                ProgressView().controlSize(.mini)
            }
        }
    }

    private func run(_ method: TravelMethod, _ row: TravelRowDisplay) {
        guard let raw = store.travelRows.first(where: { $0.accountId == row.accountId }) else { return }
        Task { await store.runTravel(method, row: raw) }
    }

    private func statusColor(_ status: TaskCenterStatus) -> Color {
        switch status {
        case .failed: return ConsoleTheme.danger
        case .claimable: return ConsoleTheme.warning
        case .unsupported, .unknown: return ConsoleTheme.tertiaryText
        default: return ConsoleTheme.secondaryText
        }
    }

    // MARK: - 页脚

    @ViewBuilder
    private var footer: some View {
        if !rows.isEmpty {
            HStack(spacing: 6) {
                Text(hostNote)
                    .font(ConsoleType.micro)
                    .foregroundStyle(ConsoleTheme.tertiaryText)
                    .lineLimit(1)
                    .help(fullHostNote)
                Spacer(minLength: 0)
            }
        }
    }

    private var hostNote: String {
        let cadence = store.travelAuto.map { "（\($0.intervalText)）" } ?? ""
        return "自动旅行仅在 AICC 运行时执行\(cadence)"
    }

    /// App 退出就没有自动旅行了 —— 这句话必须能查到，但不占正文。
    private var fullHostNote: String {
        let owner = store.travelAuto?.owner ?? "AICC 的账号助手进程"
        let lastTick = store.travelAuto?.lastTickText ?? ""
        let tail = lastTick.isEmpty ? "" : "，\(lastTick)"
        return "自动旅行由 \(owner) 执行：只在 AICC.app 运行时工作\(tail)。App 退出后不会在后台残留进程。"
    }

    // MARK: - 轮询

    /// 本地快照很便宜：助手 60 秒 tick 一次，10 秒刷新足够跟上，且不打接口。
    private func poll() async {
        #if AICC_VISUAL_TEST && AICC_CONSOLE_VISUAL
        if store.visualFixtureLoaded { return }
        #endif
        await store.loadTravel()
        while !Task.isCancelled {
            try? await Task.sleep(nanoseconds: 10 * 1_000_000_000)
            if Task.isCancelled { return }
            await store.loadTravel()
        }
    }
}
