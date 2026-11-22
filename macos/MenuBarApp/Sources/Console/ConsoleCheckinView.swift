import SwiftUI

/// 任务中心（原「签到任务」）。
///
/// 两个模式：
/// - **今日任务**：四个任务域的今日状态、手动操作与只读检查（保持原样）；
/// - **自动计划**：五类自动化一个入口看全，管理跳回对应任务域或轮换管理。
///
/// 三条纪律（与旧页面一致，不能因为改版而丢）：
/// 1. 一次只渲染**一个**任务域，不把四个模块纵向铺开；
/// 2. 只读（检查状态 / 扫描 / 重读快照）与写（签到 / 领取 / 派出）在按钮文案上分开；
/// 3. 页面加载、切平台、切任务、切模式、刷新都**不会**隐式触发任何写操作。
struct ConsoleCheckinView: View {
    @ObservedObject var store: ConsoleStore
    @EnvironmentObject private var nav: ConsoleNavigation

    @State private var showRules = false
    @State private var rotationShown = false
    @State private var rotationFocus: RotationManagerFocus = .plan

    private var mode: TaskCenterMode {
        nav.taskCenterMode
    }

    var body: some View {
        ConsolePageScaffold {
            toolbar
        } content: {
            if mode == .automation {
                ConsoleAutomationSection(store: store)
            } else {
                VStack(alignment: .leading, spacing: ConsoleTheme.sectionSpacing) {
                    // 没有账号时不摆一排 0：空状态自己会说明下一步。
                    if hasCurrentRows || currentLoading {
                        summaryRow
                    }
                    selectedSection
                }
            }
        }
        .task { await initialLoad() }
        .onAppear { consumeRotationRequest() }
        .onChange(of: nav.rotationRequest) { _, _ in consumeRotationRequest() }
        .sheet(isPresented: $rotationShown) {
            RotationManagerView(store: store, focus: rotationFocus)
        }
    }

    /// 自动计划 → 客户端轮换 → 管理。与账号页、总览共用同一条导航请求。
    private func consumeRotationRequest() {
        guard let request = nav.consumeRotationManagerRequest() else { return }
        rotationFocus = request.focus
        rotationShown = true
    }

    // MARK: - 工具栏

    private var toolbar: some View {
        HStack(spacing: 10) {
            Picker("模式", selection: $nav.taskCenterMode) {
                ForEach(TaskCenterMode.allCases) { item in
                    Text(item.title).tag(item)
                }
            }
            .pickerStyle(.segmented)
            .labelsHidden()
            .tint(ConsoleTheme.accent)
            .frame(width: 150)
            .help("今日任务看状态，自动计划看五个自动化")

            if mode == .today {
                Picker("平台", selection: $nav.checkinPlatform) {
                    ForEach(ConsoleTaskPlatform.allCases) { platform in
                        Text(platform.title).tag(platform)
                    }
                }
                .pickerStyle(.segmented)
                .labelsHidden()
                .tint(ConsoleTheme.accent)
                .frame(width: 226)
                .help("只列出真正有任务的平台：WorkBuddy CN 与 Qoder CN")

                if nav.checkinPlatform == .workbuddy {
                    Picker("任务", selection: $nav.workbuddyTask) {
                        ForEach(WorkbuddyTaskKind.allCases) { kind in
                            Text(kind.title).tag(kind)
                        }
                    }
                    .pickerStyle(.segmented)
                    .labelsHidden()
                    .tint(ConsoleTheme.accent)
                    .frame(width: 200)
                }
            }

            Spacer(minLength: 8)

            if mode == .today {
                Button {
                    showRules = true
                } label: {
                    Image(systemName: "questionmark.circle")
                        .font(.system(size: 13, weight: .medium))
                }
                .buttonStyle(ConsoleIconButtonStyle())
                .help("自动任务规则")
                .popover(isPresented: $showRules, arrowEdge: .bottom) {
                    rulesPopover
                }

                readActionButton
            }
        }
    }

    /// 当前任务域的只读入口。一个页面只有这一个「刷新状态」，不再是四个图标。
    @ViewBuilder
    private var readActionButton: some View {
        switch nav.checkinPlatform {
        case .workbuddy:
            switch nav.workbuddyTask {
            case .checkin:
                Button("检查状态") { Task { await store.refreshCheckinStatus() } }
                    .buttonStyle(ConsoleSecondaryButtonStyle())
                    .disabled(store.checkinLoading || store.checkinBatching)
                    .help("向服务端逐个账号查一次今日状态；只读，不会签到")
            case .growth:
                Button(store.growthLoading ? "扫描中…" : "扫描") { Task { await store.loadGrowth() } }
                    .buttonStyle(ConsoleSecondaryButtonStyle())
                    .disabled(store.growthLoading)
                    .help("扫描一次成长中心（只读，不会接受任务或领奖）")
            case .travel:
                Button("重读快照") { Task { await store.loadTravel() } }
                    .buttonStyle(ConsoleSecondaryButtonStyle())
                    .disabled(store.travelLoading)
                    .help("重新读取本地旅行快照，不打接口；单账号的真实状态用行内刷新")
            }
        case .qoder:
            Button("刷新") { Task { await store.refreshQoderCredits() } }
                .buttonStyle(ConsoleSecondaryButtonStyle())
                .disabled(store.qoderCreditsLoading)
                .help("读取自动领取状态与最近结果；只读，不会领取")
        }
    }

    // MARK: - 页面摘要

    private var summaryRow: some View {
        let headline = currentHeadline
        return HStack(alignment: .top, spacing: 22) {
            summaryMetric(headline.doneLabel, headline.done)
            summaryMetric(headline.pendingLabel, headline.pending)
            summaryMetric(
                headline.problemLabel,
                headline.problems,
                tone: headline.hasProblems ? ConsoleTheme.danger : ConsoleTheme.text
            )
            Spacer(minLength: 8)
        }
    }

    private func summaryMetric(_ label: String, _ value: String, tone: Color = ConsoleTheme.text) -> some View {
        VStack(alignment: .leading, spacing: 3) {
            Text(label)
                .font(ConsoleType.small)
                .foregroundStyle(ConsoleTheme.secondaryText)
                .lineLimit(1)
            Text(value)
                .font(ConsoleType.metricSmall)
                .monospacedDigit()
                .foregroundStyle(tone)
                .lineLimit(1)
        }
        .frame(width: 86, alignment: .leading)
    }

    private var currentHeadline: TaskCenterHeadline {
        switch nav.checkinPlatform {
        case .workbuddy:
            switch nav.workbuddyTask {
            case .checkin:
                return .checkin(
                    checkinRows,
                    loading: store.checkinLoading && store.checkin.rows.isEmpty
                )
            case .growth:
                return .growth(
                    growthRows,
                    claimed: growthClaimed,
                    pending: growthPending,
                    loading: store.growthLoading && store.growthResult.scans.isEmpty
                )
            case .travel:
                return .travel(
                    travelRows,
                    loading: store.travelLoading && store.travelRows.isEmpty
                )
            }
        case .qoder:
            return .credits(
                creditsRows,
                loading: store.qoderCreditsLoading && store.qoderCreditsAuto.isEmpty
            )
        }
    }

    // MARK: - 当前任务

    @ViewBuilder
    private var selectedSection: some View {
        switch nav.checkinPlatform {
        case .workbuddy:
            switch nav.workbuddyTask {
            case .checkin:
                ConsoleCheckinSection(store: store, rows: checkinRows)
            case .growth:
                ConsoleGrowthSection(store: store, rows: growthRows)
            case .travel:
                ConsoleTravelSection(store: store, rows: travelRows)
            }
        case .qoder:
            ConsoleCreditsSection(store: store, rows: creditsRows)
        }
    }

    // MARK: - 展示行（摘要与表格共用同一批）

    private var hasCurrentRows: Bool {
        switch nav.checkinPlatform {
        case .workbuddy:
            switch nav.workbuddyTask {
            case .checkin: return !checkinRows.isEmpty
            case .growth: return !growthRows.isEmpty
            case .travel: return !travelRows.isEmpty
            }
        case .qoder:
            return !creditsRows.isEmpty
        }
    }

    /// 当前任务域是不是正在读第一批数据（此时数字显示 `—`，不伪造 0）。
    private var currentLoading: Bool {
        switch nav.checkinPlatform {
        case .workbuddy:
            switch nav.workbuddyTask {
            case .checkin: return store.checkinLoading
            case .growth: return store.growthLoading
            case .travel: return store.travelLoading
            }
        case .qoder:
            return store.qoderCreditsLoading
        }
    }

    private var checkinRows: [CheckinRowDisplay] {
        CheckinRowDisplay.rows(
            store.checkin,
            acting: store.checkinActing,
            batching: store.checkinBatching
        )
    }

    private var growthRows: [GrowthAccountDisplay] {
        GrowthAccountDisplay.rows(
            store.growthResult,
            runs: store.growthRuns,
            expanded: store.expandedGrowthAccount
        )
    }

    /// 已领取的任务数：按任务状态统计，不按账号数统计（一个账号可以有多个任务）。
    private var growthClaimed: Int {
        store.growthResult.scans.reduce(0) { total, scan in
            total + scan.tasks.filter { $0.status == .claimed }.count
        }
    }

    private var growthPending: Int {
        store.growthResult.scans.reduce(0) { $0 + $1.counts.claimable + $1.counts.acceptable }
    }

    private var travelRows: [TravelRowDisplay] {
        TravelRowDisplay.rows(store.travelRows, acting: store.travelActing)
    }

    private var creditsRows: [CreditsRowDisplay] {
        CreditsRowDisplay.rows(
            items: store.qoderCreditsAuto,
            reports: store.qoderCreditsReports,
            runs: store.qoderCreditsRuns,
            busy: store.qoderCreditsBusy
        )
    }

    // MARK: - 首屏读取（全部只读）

    private func initialLoad() async {
        #if AICC_VISUAL_TEST && AICC_CONSOLE_VISUAL
        if store.visualFixtureLoaded { return }
        #endif
        async let checkin: Void = store.loadCheckin(cached: true)
        async let travel: Void = store.loadTravel()
        async let growthAuto: Void = store.loadGrowthAuto()
        async let credits: Void = store.refreshQoderCredits(silent: true)
        _ = await (checkin, travel, growthAuto, credits)
        // 成长中心仍然只在用户点「扫描」时联网：一次 9 个接口 × N 个账号。
    }

    // MARK: - 规则

    private var rulesPopover: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("自动任务规则")
                .font(ConsoleType.sectionTitle)
                .foregroundStyle(ConsoleTheme.text)
            Group {
                Text("· 每个开关只影响各自的定时检查，默认关闭；手动按钮随时可用。")
                Text("· 签到：每天 09:00（北京时间）后检查一次，服务端已签到就跳过。")
                Text("· 成长中心：自动管理只接单与领奖，不代替你完成任务；补签、兑换、抽奖不会自动执行。")
                Text("· 旅行：按开关派出与领取；Credits：按服务端活动窗口领取。")
                Text("· 所有手动操作都会先查状态再写；AICC 退出后不再自动执行。")
            }
            .font(ConsoleType.small)
            .foregroundStyle(ConsoleTheme.secondaryText)
            .fixedSize(horizontal: false, vertical: true)
        }
        .padding(14)
        .frame(width: 380)
    }
}

// MARK: - 表格共用件

enum TaskCenterColumn {
    static let spacing: CGFloat = 8
    static let inset: CGFloat = 16
}

/// 表头单元格：字号、颜色、对齐方式与额度看板一致。
struct TaskCenterHeaderCell: View {
    let title: String
    var width: CGFloat?

    var body: some View {
        Text(title)
            .font(ConsoleType.micro)
            .foregroundStyle(ConsoleTheme.tertiaryText)
            .lineLimit(1)
            .frame(width: width, alignment: .leading)
            .frame(maxWidth: width == nil ? .infinity : nil, alignment: .leading)
    }
}

/// 状态单元格：正常只留 5pt 绿点；异常用短标签 + 语义色。
/// 颜色不是唯一线索：除正常态外都有文字。
struct TaskCenterStatusCell: View {
    let status: TaskCenterStatus
    var help: String = ""

    var body: some View {
        Group {
            if status.isSilent {
                Circle()
                    .fill(ConsoleTheme.color(.ok))
                    .frame(width: 5, height: 5)
                    .help(help.isEmpty ? "正常" : help)
            } else {
                HStack(spacing: 5) {
                    Circle()
                        .fill(dotColor)
                        .frame(width: 5, height: 5)
                    Text(status.title)
                        .font(ConsoleType.caption)
                        .foregroundStyle(textColor)
                        .lineLimit(1)
                        .minimumScaleFactor(0.85)
                }
                .help(help)
            }
        }
    }

    private var dotColor: Color {
        switch status.tone {
        case .ok: return ConsoleTheme.success
        case .attention: return ConsoleTheme.warning
        case .danger: return ConsoleTheme.danger
        case .neutral: return ConsoleTheme.tertiaryText
        }
    }

    private var textColor: Color {
        switch status.tone {
        case .attention: return ConsoleTheme.warning
        case .danger: return ConsoleTheme.danger
        case .ok, .neutral: return ConsoleTheme.secondaryText
        }
    }
}
