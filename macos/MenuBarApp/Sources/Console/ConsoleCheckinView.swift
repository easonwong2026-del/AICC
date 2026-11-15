import SwiftUI

/// 签到任务页（PRD §6）：今日摘要 → 平台 → 任务类型 → 当前任务内容。
///
/// 三条纪律：
/// 1. 一次只渲染**一个**任务分区，不再把四个模块纵向铺开；
/// 2. 只读（检查状态）与写（立即签到 / 领取 / 派出）在按钮文案上分开；
/// 3. 页面刷新、切平台、切任务类型都**不会**隐式触发任何写操作。
struct ConsoleCheckinView: View {
    @ObservedObject var store: ConsoleStore
    @EnvironmentObject private var nav: ConsoleNavigation

    @State private var showRules = false

    var body: some View {
        ConsolePageScaffold {
            HStack(spacing: 8) {
                if store.tasksOverview == nil {
                    ProgressView().controlSize(.small)
                }
                Button {
                    showRules = true
                } label: {
                    Label("运行规则", systemImage: "questionmark.circle")
                }
                .buttonStyle(ConsoleSecondaryButtonStyle())
                .popover(isPresented: $showRules, arrowEdge: .bottom) {
                    rulesPopover
                }

                Spacer(minLength: 8)

                Button("刷新状态") {
                    Task { await refreshStatus() }
                }
                .buttonStyle(ConsoleSecondaryButtonStyle())
                .help("只读取本地记录与今日状态，不会签到或领取")
            }
        } content: {
            VStack(alignment: .leading, spacing: ConsoleTheme.sectionSpacing) {
                summaryCard
                platformPicker
                taskPicker
                selectedSection
            }
        }
        .task {
            if store.tasksOverview == nil {
                await store.refreshTasksOverview()
            }
            // 首屏只读本地记录：不联网、不提交任何东西。
            if store.checkin.rows.isEmpty {
                await store.loadCheckin(cached: true)
            }
        }
    }

    // MARK: 第一层：今日摘要

    private var summaryCard: some View {
        ConsoleCard(padding: 12) {
            VStack(alignment: .leading, spacing: 8) {
                HStack(alignment: .firstTextBaseline, spacing: 8) {
                    Text("今日摘要")
                        .font(ConsoleType.sectionTitle)
                        .foregroundStyle(ConsoleTheme.text)
                    if let overview = store.tasksOverview {
                        Text(overview.today)
                            .font(ConsoleType.mono)
                            .foregroundStyle(ConsoleTheme.tertiaryText)
                    }
                    Spacer(minLength: 8)
                }

                if let error = store.tasksOverviewError {
                    ConsoleInlineNotice(kind: .warning, message: error)
                } else if let overview = store.tasksOverview {
                    HStack(spacing: 20) {
                        metric("今日完成", "\(overview.doneToday)", ConsoleTheme.success)
                        metric("待完成", "\(overview.pendingToday)", ConsoleTheme.secondaryText)
                        metric("自动任务", "\(overview.autoEnabled)", ConsoleTheme.secondaryText)
                        metric(
                            "最近失败",
                            "\(overview.failureCount)",
                            overview.failureCount > 0 ? ConsoleTheme.danger : ConsoleTheme.secondaryText
                        )
                        Spacer(minLength: 0)
                    }
                    if !overview.recentFailures.isEmpty {
                        VStack(alignment: .leading, spacing: 3) {
                            ForEach(overview.recentFailures.prefix(3)) { failure in
                                HStack(spacing: 6) {
                                    Text(failure.taskLabel)
                                        .font(ConsoleType.caption)
                                        .foregroundStyle(ConsoleTheme.warning)
                                    Text(failure.displayName)
                                        .font(ConsoleType.caption)
                                        .foregroundStyle(ConsoleTheme.secondaryText)
                                        .lineLimit(1)
                                    if let detail = failure.detail {
                                        Text(detail)
                                            .font(ConsoleType.caption)
                                            .foregroundStyle(ConsoleTheme.tertiaryText)
                                            .lineLimit(1)
                                    }
                                    Spacer(minLength: 0)
                                    Text(ConsoleFormat.shortTimestamp(failure.at))
                                        .font(ConsoleType.monoSmall)
                                        .foregroundStyle(ConsoleTheme.tertiaryText)
                                }
                            }
                        }
                    }
                } else {
                    ConsoleLoadingRow(text: "读取本地任务记录…")
                }
            }
        }
    }

    private func metric(_ title: String, _ value: String, _ tint: Color) -> some View {
        VStack(alignment: .leading, spacing: 1) {
            Text(value)
                .font(ConsoleType.metricSmall)
                .monospacedDigit()
                .foregroundStyle(tint)
            Text(title)
                .font(ConsoleType.caption)
                .foregroundStyle(ConsoleTheme.tertiaryText)
        }
    }

    // MARK: 第二层：平台

    private var platformPicker: some View {
        HStack(spacing: 8) {
            ForEach(ConsoleTaskPlatform.allCases) { platform in
                let selected = nav.checkinPlatform == platform
                Button {
                    nav.checkinPlatform = platform
                } label: {
                    HStack(spacing: 7) {
                        Image(systemName: platform.symbol)
                            .font(.system(size: 11, weight: .medium))
                        Text(platform.title)
                            .font(ConsoleType.body)
                        if selected {
                            Image(systemName: "checkmark")
                                .font(.system(size: 9, weight: .bold))
                        }
                    }
                    .foregroundStyle(selected ? ConsoleTheme.text : ConsoleTheme.secondaryText)
                    .padding(.horizontal, 10)
                    .frame(height: ConsoleTheme.controlHeight)
                    .background(
                        selected ? ConsoleTheme.accentSoft : ConsoleTheme.card,
                        in: RoundedRectangle(cornerRadius: 7)
                    )
                    .overlay(
                        RoundedRectangle(cornerRadius: 7)
                            .strokeBorder(selected ? ConsoleTheme.accent.opacity(0.35) : ConsoleTheme.border,
                                          lineWidth: 1)
                    )
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
            }
            Text(taskCountNote)
                .font(ConsoleType.caption)
                .foregroundStyle(ConsoleTheme.tertiaryText)
            Spacer(minLength: 0)
        }
    }

    private var taskCountNote: String {
        switch nav.checkinPlatform {
        case .workbuddy: return "WorkBuddy AI 与 Qoder International 没有签到类任务，因此不在这里"
        case .qoder: return "只有 Qoder CN 有已验证的 Credits 活动接口"
        }
    }

    // MARK: 第三层：任务类型

    @ViewBuilder
    private var taskPicker: some View {
        switch nav.checkinPlatform {
        case .workbuddy:
            HStack(spacing: 8) {
                ForEach(WorkbuddyTaskKind.allCases) { kind in
                    taskChip(kind.title, symbol: kind.symbol, selected: nav.workbuddyTask == kind) {
                        nav.workbuddyTask = kind
                    }
                }
                Spacer(minLength: 0)
            }
        case .qoder:
            HStack(spacing: 8) {
                ForEach(QoderTaskKind.allCases) { kind in
                    taskChip(kind.title, symbol: kind.symbol, selected: true) {}
                }
                Spacer(minLength: 0)
            }
        }
    }

    private func taskChip(
        _ title: String,
        symbol: String,
        selected: Bool,
        action: @escaping () -> Void
    ) -> some View {
        Button(action: action) {
            HStack(spacing: 5) {
                Image(systemName: symbol)
                    .font(.system(size: 10))
                Text(title)
                    .font(ConsoleType.small)
            }
            .foregroundStyle(selected ? Color.white : ConsoleTheme.secondaryText)
            .padding(.horizontal, 9)
            .frame(height: 24)
            .background(
                selected ? ConsoleTheme.accent : ConsoleTheme.cardSubtle,
                in: Capsule()
            )
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }

    // MARK: 任务内容

    @ViewBuilder
    private var selectedSection: some View {
        switch nav.checkinPlatform {
        case .workbuddy:
            switch nav.workbuddyTask {
            case .checkin:
                ConsoleCheckinSection(store: store)
            case .growth:
                ConsoleGrowthSection(store: store)
            case .travel:
                ConsoleTravelSection(store: store)
            }
        case .qoder:
            ConsoleCreditsSection(store: store)
        }
    }

    // MARK: 行为

    /// 只读刷新：本地记录 + 任务汇总，不发写请求。
    private func refreshStatus() async {
        async let overview: Void = store.refreshTasksOverview()
        async let auto: Void = store.loadCheckinAuto()
        _ = await (overview, auto)
        await store.loadCheckin(cached: true, silent: true)
    }

    private var rulesPopover: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("自动任务运行规则")
                .font(ConsoleType.sectionTitle)
                .foregroundStyle(ConsoleTheme.text)
            Group {
                Text("· 每个账号一个开关，默认关闭；只有开关打开且 AICC 在运行时才会自动执行。")
                Text("· WorkBuddy 签到每天 09:00（北京时间）后检查一次，服务端已签到就跳过。")
                Text("· 成长中心可自动接单与领奖；Buddy 旅行按开关派出与领取。")
                Text("· Qoder Credits 按服务端活动窗口，自动开关默认关闭。")
                Text("· 手动按钮不受自动开关限制，且都会先查状态再写。")
            }
            .font(ConsoleType.small)
            .foregroundStyle(ConsoleTheme.secondaryText)
            .fixedSize(horizontal: false, vertical: true)
        }
        .padding(14)
        .frame(width: 360)
    }
}
