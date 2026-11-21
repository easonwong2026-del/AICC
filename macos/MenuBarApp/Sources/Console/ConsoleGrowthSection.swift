import SwiftUI

/// 成长中心（WorkBuddy CN）。
///
/// 只读扫描 + 一个全局自动管理开关。表格按账号折叠：首屏只回答「谁有任务、谁要登录」，
/// 任务明细展开后一层平铺，不再给每个任务做卡片。
struct ConsoleGrowthSection: View {
    @ObservedObject var store: ConsoleStore
    let rows: [GrowthAccountDisplay]
    @EnvironmentObject private var nav: ConsoleNavigation

    private enum Column {
        static let task: CGFloat = 132
        static let lastRun: CGFloat = 150
        static let status: CGFloat = 88
        static let chevron: CGFloat = 16
        static let accountMin: CGFloat = 160
    }

    var body: some View {
        VStack(alignment: .leading, spacing: ConsoleTheme.groupSpacing) {
            ConsoleSectionHeader(title: "成长任务", detail: summary) { EmptyView() }
            automationRow
            notices
            content
        }
    }

    private var scanned: Bool {
        store.lastGrowthScan != nil || !store.growthResult.scans.isEmpty
    }

    private var summary: String {
        TaskCenterSummary.growth(
            rows,
            claimable: store.growthResult.overview.claimable,
            auto: store.growthAuto,
            scanned: scanned
        )
    }

    // MARK: - 自动管理

    private var automationRow: some View {
        HStack(spacing: 10) {
            Toggle("自动管理", isOn: Binding(
                get: { store.growthAuto.enabled },
                set: { enabled in
                    Task { await store.setGrowthAutoEnabled(enabled) }
                }
            ))
            .toggleStyle(.switch)
            .controlSize(.small)
            .font(ConsoleType.small)
            .disabled(store.growthActing || store.growthAuto.running)

            Text(autoDetail)
                .font(ConsoleType.micro)
                .foregroundStyle(ConsoleTheme.tertiaryText)
                .lineLimit(1)
                .help(store.growthAuto.modeHelp)

            if store.growthAuto.running || store.growthActing {
                ProgressView().controlSize(.small)
            }

            Spacer(minLength: 8)

            Button("立即检查") {
                Task { await store.runGrowthTick(force: true) }
            }
            .buttonStyle(
                store.growthAuto.tickEmphasis == .primary
                    ? AnyButtonStyle(ConsolePrimaryButtonStyle())
                    : AnyButtonStyle(ConsoleSecondaryButtonStyle())
            )
            .disabled(store.growthActing || store.growthAuto.running)
            .help(store.growthAuto.enabled
                  ? "跑一轮：接单 + 领奖；不会补签、兑换或抽奖"
                  : "开关关着时只做只读检查，不会接受任务或领取奖励")
        }
    }

    private var autoDetail: String {
        guard store.growthAuto.enabled else { return "开启后自动接单与领奖" }
        return "\(store.growthAuto.intervalText) · \(store.growthAuto.nextTickText)"
    }

    // MARK: - 提示

    @ViewBuilder
    private var notices: some View {
        if let notice = store.growthNotice {
            ConsoleInlineNotice(kind: .info, message: notice)
        }
        if let error = store.growthError, !rows.isEmpty {
            ConsoleInlineNotice(kind: .error, message: error, actionTitle: "重试") {
                Task { await store.loadGrowth() }
            }
        }
    }

    // MARK: - 表格

    @ViewBuilder
    private var content: some View {
        if rows.isEmpty {
            if store.growthLoading {
                ConsoleLoadingRow(text: "正在扫描成长中心")
            } else if let error = store.growthError {
                ConsoleEmptyState(
                    symbol: "exclamationmark.triangle",
                    title: "扫描失败",
                    message: error,
                    actionTitle: "重试"
                ) {
                    Task { await store.loadGrowth() }
                }
            } else if scanned {
                ConsoleEmptyState(
                    symbol: "person.2",
                    title: "没有可扫描的账号",
                    message: "WorkBuddy CN 账号添加后，成长任务会出现在这里。",
                    actionTitle: "打开 AI 账号"
                ) {
                    nav.page = .accounts
                }
            } else {
                ConsoleEmptyState(
                    symbol: "sparkles",
                    title: "还没扫描",
                    message: "扫描一次成长中心，只读：不会接受任务，也不会领奖。",
                    actionTitle: "扫描"
                ) {
                    Task { await store.loadGrowth() }
                }
            }
        } else {
            ConsoleListSurface(horizontalPadding: 0) {
                VStack(spacing: 0) {
                    header
                    Divider().overlay(ConsoleTheme.border)
                    ForEach(Array(rows.enumerated()), id: \.element.id) { index, row in
                        accountBlock(row)
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
            TaskCenterHeaderCell(title: "任务", width: Column.task)
            TaskCenterHeaderCell(title: "最近执行", width: Column.lastRun)
            TaskCenterHeaderCell(title: "状态", width: Column.status)
            Color.clear.frame(width: Column.chevron, height: 1)
        }
        .padding(.horizontal, TaskCenterColumn.inset)
        .padding(.vertical, 8)
    }

    private func accountBlock(_ row: GrowthAccountDisplay) -> some View {
        VStack(spacing: 0) {
            Button {
                store.toggleGrowthAccount(row.accountId)
            } label: {
                HStack(spacing: TaskCenterColumn.spacing) {
                    VStack(alignment: .leading, spacing: 2) {
                        Text(row.name)
                            .font(ConsoleType.rowTitle)
                            .foregroundStyle(ConsoleTheme.text)
                            .lineLimit(1)
                            .truncationMode(.middle)
                            .help(row.name)
                        if !row.profileText.isEmpty {
                            Text(row.profileText)
                                .font(ConsoleType.micro)
                                .foregroundStyle(ConsoleTheme.tertiaryText)
                                .lineLimit(1)
                                .help(row.profileText)
                        }
                    }
                    .frame(minWidth: Column.accountMin, maxWidth: .infinity, alignment: .leading)

                    VStack(alignment: .leading, spacing: 2) {
                        Text(row.taskText)
                            .font(ConsoleType.body)
                            .foregroundStyle(ConsoleTheme.secondaryText)
                            .lineLimit(1)
                        if !row.pendingText.isEmpty {
                            Text(row.pendingText)
                                .font(ConsoleType.micro)
                                .foregroundStyle(row.pendingIsAttention ? ConsoleTheme.warning : ConsoleTheme.tertiaryText)
                                .lineLimit(1)
                        }
                    }
                    .frame(width: Column.task, alignment: .leading)

                    Text(row.lastRunText)
                        .font(ConsoleType.micro)
                        .foregroundStyle(ConsoleTheme.tertiaryText)
                        .lineLimit(1)
                        .help(row.lastRunText)
                        .frame(width: Column.lastRun, alignment: .leading)

                    TaskCenterStatusCell(status: row.status, help: row.help)
                        .frame(width: Column.status, alignment: .leading)

                    Image(systemName: row.isExpanded ? "chevron.down" : "chevron.right")
                        .font(.system(size: 10, weight: .semibold))
                        .foregroundStyle(ConsoleTheme.tertiaryText)
                        .frame(width: Column.chevron)
                }
                .padding(.horizontal, TaskCenterColumn.inset)
                .padding(.vertical, 8)
                .frame(minHeight: ConsoleTheme.rowHeight)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)

            if row.isExpanded {
                taskList(row)
            }
        }
    }

    private func taskList(_ row: GrowthAccountDisplay) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            if row.tasks.isEmpty {
                Text(row.status == .needsLogin
                     ? "这个账号没有可用授权，先到「AI 账号」重新登录或从 Magpie 导入。"
                     : "服务端没有下发任务。")
                    .font(ConsoleType.micro)
                    .foregroundStyle(ConsoleTheme.tertiaryText)
                    .padding(.leading, 28)
                    .padding(.vertical, 8)
            } else {
                ForEach(row.tasks) { task in
                    taskRow(task)
                }
            }
        }
        .padding(.bottom, 4)
        .background(ConsoleTheme.cardSubtle)
    }

    private func taskRow(_ task: GrowthTaskDisplay) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Text(task.title)
                    .font(ConsoleType.small.weight(.medium))
                    .foregroundStyle(ConsoleTheme.text)
                    .lineLimit(1)
                    .layoutPriority(1)

                Text("·")
                    .font(ConsoleType.micro)
                    .foregroundStyle(ConsoleTheme.tertiaryText)

                Text(task.condition)
                    .font(ConsoleType.micro)
                    .foregroundStyle(ConsoleTheme.tertiaryText)
                    .lineLimit(1)
                    .truncationMode(.tail)
                    .help(task.condition)

                Spacer(minLength: 8)

                Text(task.progress)
                    .font(ConsoleType.monoSmall)
                    .foregroundStyle(ConsoleTheme.secondaryText)
                    .lineLimit(1)
                    .frame(width: 48, alignment: .trailing)

                Text(task.reward)
                    .font(ConsoleType.micro)
                    .foregroundStyle(ConsoleTheme.secondaryText)
                    .lineLimit(1)
                    .minimumScaleFactor(0.9)
                    .frame(width: 112, alignment: .trailing)

                // 类型只在「需人工 / 待识别」时出现，放在状态左边，避免插进标题里读成一句话。
                Text(task.classLabel ?? "")
                    .font(ConsoleType.micro)
                    .foregroundStyle(task.status == .locked ? ConsoleTheme.tertiaryText : ConsoleTheme.warning)
                    .lineLimit(1)
                    .frame(width: 40, alignment: .trailing)
                    .help(task.classLabel == nil ? "" : task.classReason)

                TaskCenterStatusCell(status: task.status, help: task.classReason)
                    .frame(width: 88, alignment: .leading)
            }

            if let problem = task.problemText {
                Text(problem)
                    .font(ConsoleType.micro)
                    .foregroundStyle(ConsoleTheme.danger)
                    .lineLimit(1)
                    .help(problem)
            }
        }
        .padding(.leading, 28)
        .padding(.trailing, 32)
        .padding(.vertical, 5)
    }
}

/// `if/else` 里切换两种 ButtonStyle 需要一个类型擦除的壳（只在本文件使用）。
private struct AnyButtonStyle: ButtonStyle {
    private let makeBodyClosure: (Configuration) -> AnyView

    init<S: ButtonStyle>(_ style: S) {
        makeBodyClosure = { configuration in
            AnyView(style.makeBody(configuration: configuration))
        }
    }

    func makeBody(configuration: Configuration) -> some View {
        makeBodyClosure(configuration)
    }
}
