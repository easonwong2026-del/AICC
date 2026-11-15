import SwiftUI

/// 签到任务页里的 **Buddy 旅行** 区域。
///
/// 设计取向：一屏看完每个账号「现在在哪、要不要派、有没有可领的奖励」，
/// 每个账号一个开关 + 两个按钮，不再新增菜单或弹窗。
///
/// 数据来源：`travel.summary`（本地快照，10 秒轮询一次，不打接口）。
/// 只有点「派出 / 领取 / 刷新状态」才会真的请求服务端 —— 界面绝不偷偷提交。
struct ConsoleTravelSection: View {
    @ObservedObject var store: ConsoleStore

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            header
            card
            footerNote
        }
        .task { await poll() }
    }

    // MARK: - 头部

    private var header: some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            ConsoleSectionTitle(
                title: "Buddy 旅行",
                detail: detailText
            )
            Spacer(minLength: 8)
            if let auto = store.travelAuto {
                ConsoleBadge(
                    text: auto.enabledAccounts > 0 ? "自动 \(auto.enabledAccounts) 个账号" : "自动未开启",
                    tint: auto.enabledAccounts > 0 ? ConsoleTheme.accent : ConsoleTheme.tertiaryText
                )
            }
            Button {
                Task { await store.loadTravel() }
            } label: {
                Image(systemName: "arrow.clockwise")
                    .font(.system(size: 11, weight: .medium))
            }
            .buttonStyle(ConsoleIconButtonStyle())
            .help("重新读取旅行快照（不打接口）")
            .disabled(store.travelLoading)
        }
    }

    private var detailText: String {
        if let error = store.travelError { return error }
        if let auto = store.travelAuto { return auto.intervalText }
        return "尚未读取"
    }

    // MARK: - 卡片

    private var card: some View {
        ConsoleCard(padding: 0) {
            if store.travelRows.isEmpty {
                emptyState
            } else {
                VStack(spacing: 0) {
                    ForEach(Array(store.travelRows.enumerated()), id: \.element.id) { index, row in
                        accountRow(row)
                        if index < store.travelRows.count - 1 {
                            Divider().overlay(ConsoleTheme.border).padding(.leading, 16)
                        }
                    }
                }
            }
        }
    }

    @ViewBuilder
    private var emptyState: some View {
        if let error = store.travelError {
            ConsoleEmptyState(
                symbol: "exclamationmark.triangle",
                title: "读不到旅行状态",
                message: error
            )
        } else {
            ConsoleEmptyState(
                symbol: "airplane",
                title: "没有 WorkBuddy 账号",
                message: "先去「AI 账号」页添加账号（可以从 Magpie 导入），旅行才会出现在这里。"
            )
        }
    }

    private func accountRow(_ row: TravelAccountRow) -> some View {
        let status = row.status
        let acting = store.travelActing == row.accountId
        return VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 10) {
                VStack(alignment: .leading, spacing: 2) {
                    HStack(spacing: 6) {
                        Text(row.nickname)
                            .font(.system(size: 12, weight: .medium))
                            .foregroundStyle(ConsoleTheme.text)
                            .lineLimit(1)
                        if row.isBorrowed {
                            ConsoleBadge(text: "Magpie", tint: ConsoleTheme.tertiaryText)
                        }
                    }
                    Text(status?.secondaryText ?? "还没检查")
                        .font(.system(size: 11))
                        .foregroundStyle(ConsoleTheme.tertiaryText)
                        .lineLimit(1)
                }
                .frame(width: 168, alignment: .leading)

                ConsoleBadge(text: status?.stageText ?? "待检查", tint: tone(for: status))
                    .frame(width: 74, alignment: .leading)

                Text(status?.destinationText ?? "—")
                    .font(.system(size: 11))
                    .foregroundStyle(ConsoleTheme.secondaryText)
                    .lineLimit(1)
                    .truncationMode(.tail)

                Spacer(minLength: 8)

                Toggle(
                    "",
                    isOn: Binding(
                        get: { row.travelEnabled },
                        set: { newValue in
                            Task { await store.setTravelEnabled(row, enabled: newValue) }
                        }
                    )
                )
                .labelsHidden()
                .toggleStyle(.switch)
                .controlSize(.small)
                .disabled(acting)
                .help("开启后由 AICC 常驻助手自动派出并领取")

                travelButton(
                    title: "领取",
                    enabled: status?.canClaim == true && !acting,
                    prominent: status?.canClaim == true
                ) {
                    Task { await store.runTravel(.claim, row: row) }
                }

                travelButton(
                    title: "派出",
                    enabled: status?.canDepart == true && !acting,
                    prominent: false
                ) {
                    Task { await store.runTravel(.depart, row: row) }
                }

                Button {
                    Task { await store.runTravel(.refresh, row: row) }
                } label: {
                    Image(systemName: "arrow.triangle.2.circlepath")
                        .font(.system(size: 11, weight: .medium))
                }
                .buttonStyle(ConsoleIconButtonStyle())
                .disabled(acting)
                .help("查一次真实状态")
            }

            if let warning = status?.warningText {
                Text(warning)
                    .font(.system(size: 11))
                    .foregroundStyle(ConsoleTheme.warning)
                    .lineLimit(2)
                    .padding(.leading, 2)
            }
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 10)
    }

    @ViewBuilder
    private func travelButton(
        title: String,
        enabled: Bool,
        prominent: Bool,
        action: @escaping () -> Void
    ) -> some View {
        if prominent {
            Button(title, action: action)
                .buttonStyle(ConsolePrimaryButtonStyle(enabled: enabled))
                .disabled(!enabled)
                .frame(width: 46)
        } else {
            Button(title, action: action)
                .buttonStyle(ConsoleSecondaryButtonStyle())
                .disabled(!enabled)
                .frame(width: 46)
        }
    }

    private func tone(for status: TravelStatus?) -> Color {
        switch status?.stage {
        case .dispatchable: return ConsoleTheme.accent
        case .traveling: return ConsoleTheme.accent
        case .arrived: return ConsoleTheme.warning
        case .doneToday: return ConsoleTheme.success
        case .noBuddy, .unsupported: return ConsoleTheme.tertiaryText
        case .failed: return ConsoleTheme.danger
        case .unknown, .none: return ConsoleTheme.tertiaryText
        }
    }

    // MARK: - 底部说明

    private var footerNote: some View {
        VStack(alignment: .leading, spacing: 3) {
            if let notice = store.travelNotice {
                Text(notice)
                    .font(.system(size: 11))
                    .foregroundStyle(ConsoleTheme.secondaryText)
            }
            Text(hostNote)
                .font(.system(size: 11))
                .foregroundStyle(ConsoleTheme.tertiaryText)
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    /// 必须如实说明依赖关系：App 退出就没有自动旅行了。
    private var hostNote: String {
        let owner = store.travelAuto?.owner ?? "AICC 的账号助手进程"
        let lastTick = store.travelAuto?.lastTickText ?? ""
        let tail = lastTick.isEmpty ? "" : "，\(lastTick)"
        return "自动旅行由 \(owner) 执行：只在 AICC.app 运行时工作\(tail)。App 退出后不会在后台残留进程。"
    }

    // MARK: - 轮询

    /// 本地快照很便宜，但也不必太勤：助手 60 秒 tick 一次，10 秒刷新足够跟上。
    private func poll() async {
        await store.loadTravel()
        while !Task.isCancelled {
            try? await Task.sleep(nanoseconds: 10 * 1_000_000_000)
            if Task.isCancelled { return }
            await store.loadTravel()
        }
    }
}
