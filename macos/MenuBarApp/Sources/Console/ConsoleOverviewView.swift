import SwiftUI

/// 总览只回答：整体是否正常、今天是否完成、现在要处理什么。
/// 详细额度、任务日志和授权机制分别留在对应页面。
struct ConsoleOverviewView: View {
    @ObservedObject var store: ConsoleStore
    @EnvironmentObject private var nav: ConsoleNavigation

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: ConsoleTheme.sectionSpacing) {
                summarySection
                errorSection
                attentionSection
                todaySection
                ViewThatFits(in: .horizontal) {
                    HStack(alignment: .top, spacing: ConsoleTheme.sectionSpacing) {
                        platformSection
                        activitySection.frame(width: 300)
                    }
                    VStack(alignment: .leading, spacing: ConsoleTheme.sectionSpacing) {
                        platformSection
                        activitySection
                    }
                }
                recentSection
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.horizontal, ConsoleTheme.pagePadding)
            .padding(.top, ConsoleTheme.pageTopPadding)
            .padding(.bottom, ConsoleTheme.pageBottomPadding)
        }
        .task {
            if store.tasksOverview == nil { await store.refreshTasksOverview() }
            if store.checkin.rows.isEmpty { await store.loadCheckin(cached: true) }
            if store.travelRows.isEmpty { await store.loadTravel() }
            if store.lastActivityRefresh == nil && store.activityError == nil && !store.activityLoading {
                await store.loadActivity()
            }
        }
    }

    // MARK: - 状态摘要

    /// 首屏的“需要处理”汇总：轮换未知与恢复异常都不能被当成正常。
    private var attention: ConsoleOverviewAttention {
        ConsoleOverviewAttention(
            fleetIssues: store.fleet.issues,
            failureCount: store.tasksOverview?.failureCount ?? 0,
            accountsError: store.accountsError != nil,
            qoderError: store.qoderError != nil,
            rotation: ConsoleRotationAttentionState(
                status: store.wbRotationStatus,
                error: store.wbRotationError
            )
        )
    }

    private var attentionCount: Int { attention.count }

    private var summarySection: some View {
        ViewThatFits(in: .horizontal) {
            HStack(alignment: .center, spacing: 24) {
                summaryHeadline
                Spacer(minLength: 16)
                summaryMetrics.frame(width: 310)
            }
            VStack(alignment: .leading, spacing: 20) {
                summaryHeadline
                summaryMetrics
            }
        }
        .padding(.bottom, 4)
    }

    private var summaryHeadline: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(headline)
                .font(ConsoleType.hero)
                .foregroundStyle(ConsoleTheme.text)
                .lineLimit(2)
                .fixedSize(horizontal: false, vertical: true)
            Text(summaryDetail)
                .font(ConsoleType.body)
                .foregroundStyle(ConsoleTheme.secondaryText)
                .lineLimit(2)
                .fixedSize(horizontal: false, vertical: true)
        }
        .frame(minWidth: 260, alignment: .leading)
    }

    private var headline: String {
        if store.loadingAccounts && store.fleet.serviceAccounts == 0 { return "正在读取账号" }
        if store.fleet.serviceAccounts == 0 { return "从添加账号开始" }
        // 只有轮换状态已确认无异常时才可能出现“一切正常”。
        return attention.headline
    }

    private var summaryDetail: String {
        let healthy = store.fleet.healthy
        let total = store.fleet.serviceAccounts
        let accounts = total == 0 ? "添加账号后即可管理授权与任务" : "\(total) 个账号，\(healthy) 个授权正常"
        guard let overview = store.tasksOverview else { return accounts }
        return "\(accounts) · 今日完成 \(overview.doneToday) 项"
    }

    private var summaryMetrics: some View {
        HStack(alignment: .top, spacing: 16) {
            ConsoleSummaryMetric(
                label: "账号",
                value: "\(store.fleet.serviceAccounts)",
                detail: store.fleet.localProfiles > 0 ? "本机配置 \(store.fleet.localProfiles)" : nil
            )
            ConsoleSummaryMetric(
                label: "今日完成",
                value: store.tasksOverview.map { "\($0.doneToday)" } ?? "—",
                detail: store.tasksOverview.map { "待完成 \($0.pendingToday)" } ?? "尚未读取"
            )
            ConsoleSummaryMetric(
                label: "需要处理",
                value: "\(attentionCount)",
                detail: attentionCount > 0 ? "查看下方事项" : "无需操作",
                tone: attentionCount > 0 ? ConsoleTheme.warning : ConsoleTheme.text
            )
        }
    }

    @ViewBuilder
    private var errorSection: some View {
        if store.loadingAccounts && store.accounts.isEmpty {
            ConsoleLoadingRow(text: "正在读取账号状态")
        }
        if let error = store.tasksOverviewError {
            ConsoleInlineNotice(kind: .warning, message: "今日任务暂时无法读取：\(error)")
        }
    }

    // MARK: - 待处理

    private struct PendingItem: Identifiable {
        let id: String
        let title: String
        let detail: String
        let selection: ConsoleAccountSelection?
        let filter: ConsolePlatformFilter
        let destination: ConsolePage
        /// 点开后是否直接打开 WorkBuddy 登录与夜间轮换管理面板（只导航，不执行任何恢复）。
        var opensRotationSheet = false
    }

    private var pendingItems: [PendingItem] {
        var items: [PendingItem] = []
        // P0：轮换上一轮没有恢复成功、或状态读取失败时，必须出现在首屏待处理里。
        // 点击只带上下文进入账号页的轮换管理面板，恢复仍走原有确认流程。
        switch attention.rotation {
        case .needsAction(let run):
            items.append(PendingItem(
                id: "wb-rotation-recovery",
                title: attention.rotation.pendingTitle ?? "WorkBuddy 轮换未恢复",
                detail: run.restoreText,
                selection: nil,
                filter: .workbuddy,
                destination: .accounts,
                opensRotationSheet: true
            ))
        case .unavailable:
            items.append(PendingItem(
                id: "wb-rotation-unknown",
                title: attention.rotation.pendingTitle ?? "轮换状态无法确认",
                detail: attention.rotation.pendingDetail ?? "状态读取失败",
                selection: nil,
                filter: .workbuddy,
                destination: .accounts,
                opensRotationSheet: true
            ))
        case .loading, .clear:
            break
        }
        for account in store.accounts where account.displayAuthTone == .error || (account.isBorrowed && account.magpieAuthorization?.effectiveStatus == "unavailable") {
            items.append(PendingItem(
                id: "wb-\(account.id)",
                title: account.nickname,
                detail: "\(account.platformName) · \(account.displayAuthLabel)",
                selection: .workbuddy(account.id),
                filter: account.isWorkbuddyAI ? .workbuddyAI : .workbuddy,
                destination: .accounts
            ))
        }
        for account in store.qoderServiceAccounts where account.usableSources.isEmpty {
            items.append(PendingItem(
                id: "qoder-\(account.id)",
                title: account.displayName,
                detail: "\(account.editionName) · \(account.statusLabel)",
                selection: .qoderService(account.id),
                filter: account.edition == "qoder_cn" ? .qoder : .qoderGlobal,
                destination: .accounts
            ))
        }
        for failure in store.tasksOverview?.recentFailures ?? [] {
            items.append(PendingItem(
                id: "failure-\(failure.id)",
                title: failure.displayName,
                detail: "\(failure.taskLabel) · \(failure.detail ?? failure.status)",
                selection: nil,
                filter: .all,
                destination: .checkin
            ))
        }
        if let error = store.accountsError {
            let detail: String
            if let lastSuccess = store.lastAccountsRefresh {
                detail = "当前保留旧快照（上次成功 \(ConsoleFormat.shortTime(lastSuccess))，数据已过期）· \(error)"
            } else {
                detail = error
            }
            items.append(PendingItem(id: "accounts-error", title: "账号更新失败", detail: detail, selection: nil, filter: .all, destination: .accounts))
        }
        if let error = store.qoderError {
            items.append(PendingItem(id: "qoder-error", title: "Qoder 更新失败", detail: error, selection: nil, filter: .all, destination: .accounts))
        }
        return items
    }

    private var attentionSection: some View {
        VStack(alignment: .leading, spacing: ConsoleTheme.groupSpacing) {
            ConsoleSectionHeader(title: "需要处理", detail: pendingItems.isEmpty ? nil : "\(pendingItems.count) 项") {
                EmptyView()
            }
            if pendingItems.isEmpty {
                Label("没有需要处理的事项", systemImage: "checkmark.circle")
                    .font(ConsoleType.body)
                    .foregroundStyle(ConsoleTheme.secondaryText)
                    .padding(.vertical, 4)
            } else {
                ConsoleListSurface {
                    VStack(spacing: 0) {
                        ForEach(Array(pendingItems.prefix(4).enumerated()), id: \.element.id) { index, item in
                            Button { open(item) } label: {
                                HStack(spacing: 12) {
                                    Image(systemName: "exclamationmark.circle.fill")
                                        .foregroundStyle(ConsoleTheme.warning)
                                    VStack(alignment: .leading, spacing: 2) {
                                        Text(item.title)
                                            .font(ConsoleType.rowTitle)
                                            .foregroundStyle(ConsoleTheme.text)
                                            .lineLimit(1)
                                        Text(item.detail)
                                            .font(ConsoleType.caption)
                                            .foregroundStyle(ConsoleTheme.secondaryText)
                                            .lineLimit(2)
                                    }
                                    Spacer(minLength: 8)
                                    Image(systemName: "chevron.right")
                                        .font(ConsoleType.caption)
                                        .foregroundStyle(ConsoleTheme.tertiaryText)
                                }
                                .frame(minHeight: ConsoleTheme.rowHeight)
                                .padding(.vertical, 6)
                                .contentShape(Rectangle())
                            }
                            .buttonStyle(.plain)
                            if index < min(4, pendingItems.count) - 1 {
                                Divider().overlay(ConsoleTheme.border)
                            }
                        }
                    }
                }
            }
        }
    }

    private func open(_ item: PendingItem) {
        if item.destination == .accounts {
            nav.openAccounts(filter: item.filter, selection: item.selection)
            if item.opensRotationSheet { nav.requestRotationManager(.recovery) }
        } else {
            nav.openCheckin(platform: .workbuddy)
        }
    }

    // MARK: - 今日任务

    private struct TaskSummary: Identifiable {
        let id: String
        let title: String
        let value: String
        let detail: String
        let tone: Color?
        let platform: ConsoleTaskPlatform
        let workbuddy: WorkbuddyTaskKind
    }

    private var taskSummaries: [TaskSummary] {
        guard let overview = store.tasksOverview else { return [] }
        return [
            TaskSummary(
                id: "checkin", title: "每日签到", value: "\(overview.checkinDone) 完成",
                detail: overview.checkinFailed > 0 ? "\(overview.checkinFailed) 失败" : "待完成 \(overview.checkinPending)",
                tone: overview.checkinFailed > 0 ? ConsoleTheme.danger : nil,
                platform: .workbuddy, workbuddy: .checkin
            ),
            TaskSummary(
                id: "growth", title: "成长任务", value: "\(overview.growthRuns) 次执行",
                detail: overview.growthFailed > 0 ? "\(overview.growthFailed) 失败" : "自动开启 \(overview.autoEnabled)",
                tone: overview.growthFailed > 0 ? ConsoleTheme.danger : nil,
                platform: .workbuddy, workbuddy: .growth
            ),
            TaskSummary(
                id: "travel", title: "Buddy 旅行", value: "\(overview.travelClaimed) 已领取",
                detail: "待领取 \(overview.travelPending)", tone: nil,
                platform: .workbuddy, workbuddy: .travel
            ),
            TaskSummary(
                id: "credits", title: "每日 Credits", value: "\(overview.qoderClaimed) 已领取",
                detail: overview.qoderFailed > 0 ? "\(overview.qoderFailed) 失败" : (overview.qoderClaimable.map { "可领取 \($0)" } ?? "尚未查询"),
                tone: overview.qoderFailed > 0 ? ConsoleTheme.danger : nil,
                platform: .qoder, workbuddy: .checkin
            ),
        ]
    }

    private var todaySection: some View {
        VStack(alignment: .leading, spacing: ConsoleTheme.groupSpacing) {
            ConsoleSectionHeader(title: "今日任务", detail: nil) {
                Button("查看全部") { nav.openCheckin(platform: .workbuddy) }
                    .buttonStyle(.borderless)
                    .font(ConsoleType.small)
            }
            if store.tasksOverviewLoading && store.tasksOverview == nil {
                ConsoleLoadingRow(text: "正在读取今日任务")
            } else if taskSummaries.isEmpty {
                Text("还没有今日任务记录")
                    .font(ConsoleType.body)
                    .foregroundStyle(ConsoleTheme.secondaryText)
            } else {
                ConsoleListSurface {
                    ViewThatFits(in: .horizontal) {
                        HStack(spacing: 0) {
                            ForEach(Array(taskSummaries.enumerated()), id: \.element.id) { index, task in
                                taskCell(task)
                                if index < taskSummaries.count - 1 {
                                    Divider().overlay(ConsoleTheme.border).frame(height: 54)
                                }
                            }
                        }
                        LazyVGrid(columns: [GridItem(.flexible()), GridItem(.flexible())], spacing: 18) {
                            ForEach(taskSummaries) { task in taskCell(task) }
                        }
                        .padding(.vertical, 8)
                    }
                }
            }
        }
    }

    private func taskCell(_ task: TaskSummary) -> some View {
        Button {
            nav.openCheckin(platform: task.platform, workbuddy: task.workbuddy)
        } label: {
            VStack(alignment: .leading, spacing: 4) {
                Text(task.title)
                    .font(ConsoleType.small)
                    .foregroundStyle(ConsoleTheme.secondaryText)
                Text(task.value)
                    .font(ConsoleType.rowTitle)
                    .foregroundStyle(ConsoleTheme.text)
                    .lineLimit(1)
                Text(task.detail)
                    .font(ConsoleType.caption)
                    .foregroundStyle(task.tone ?? ConsoleTheme.tertiaryText)
                    .lineLimit(1)
            }
            .frame(maxWidth: .infinity, minHeight: 64, alignment: .leading)
            .padding(.horizontal, 12)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }

    // MARK: - 平台与活跃

    private var platformSection: some View {
        VStack(alignment: .leading, spacing: ConsoleTheme.groupSpacing) {
            ConsoleSectionHeader(title: "账号分布", detail: nil) { EmptyView() }
            ConsoleListSurface {
                VStack(spacing: 0) {
                    ForEach(Array(store.fleet.cards.enumerated()), id: \.element.id) { index, card in
                        Button { nav.openAccounts(filter: card.kind.filter) } label: {
                            HStack(spacing: 12) {
                                Image(systemName: card.kind.symbol)
                                    .font(ConsoleType.body)
                                    .foregroundStyle(ConsoleTheme.secondaryText)
                                    .frame(width: 20)
                                Text(card.kind.title)
                                    .font(ConsoleType.rowTitle)
                                    .foregroundStyle(ConsoleTheme.text)
                                    .lineLimit(1)
                                Spacer(minLength: 8)
                                Text("\(card.accounts)")
                                    .font(ConsoleType.body)
                                    .monospacedDigit()
                                    .foregroundStyle(ConsoleTheme.text)
                                if card.issues > 0 {
                                    ConsoleBadge(text: "\(card.issues) 异常", tint: ConsoleTheme.warning)
                                }
                                Image(systemName: "chevron.right")
                                    .font(ConsoleType.caption)
                                    .foregroundStyle(ConsoleTheme.tertiaryText)
                            }
                            .frame(minHeight: ConsoleTheme.rowHeight)
                            .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                        if index < store.fleet.cards.count - 1 { Divider().overlay(ConsoleTheme.border) }
                    }
                }
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private var activitySection: some View {
        VStack(alignment: .leading, spacing: ConsoleTheme.groupSpacing) {
            ConsoleSectionHeader(title: "成长活跃", detail: nil) {
                Button {
                    Task { await store.loadActivity() }
                } label: {
                    Image(systemName: "arrow.clockwise")
                }
                .buttonStyle(.borderless)
                .disabled(store.activityLoading)
                .help("重新查询今日活跃，不执行任务")
            }
            if store.activityLoading && store.activity.accounts.isEmpty {
                ConsoleLoadingRow(text: "正在查询")
            } else if store.activity.accounts.isEmpty {
                Text("尚未查询今日活跃")
                    .font(ConsoleType.body)
                    .foregroundStyle(ConsoleTheme.secondaryText)
            } else {
                let active = store.activity.accounts.filter { $0.todayActive == true }.count
                HStack(spacing: 20) {
                    ConsoleSummaryMetric(label: "今日活跃", value: "\(active)/\(store.activity.accounts.count)")
                    ConsoleSummaryMetric(
                        label: "需要行动",
                        value: "\(store.activity.needsAction.count)",
                        tone: store.activity.needsAction.isEmpty ? ConsoleTheme.text : ConsoleTheme.warning
                    )
                }
                if let failure = store.activityError {
                    Text(failure)
                        .font(ConsoleType.caption)
                        .foregroundStyle(ConsoleTheme.warning)
                        .lineLimit(2)
                }
                ForEach(store.activity.needsAction.prefix(2)) { account in
                    HStack(spacing: 8) {
                        Text(account.nickname)
                            .font(ConsoleType.small)
                            .foregroundStyle(ConsoleTheme.text)
                            .lineLimit(1)
                        Spacer(minLength: 4)
                        Text(account.diagnosis.shortLabel)
                            .font(ConsoleType.caption)
                            .foregroundStyle(ConsoleTheme.warning)
                            .lineLimit(1)
                    }
                    .help(account.diagnosis.action)
                }
                if store.activity.needsAction.contains(where: { $0.diagnosis.needsClient }) {
                    Button("打开 WorkBuddy") { WorkBuddyClient.open() }
                        .buttonStyle(ConsoleSecondaryButtonStyle())
                        .disabled(!WorkBuddyClient.isInstalled)
                }
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    // MARK: - 最近活动

    private struct RecentRecord: Identifiable {
        let id: String
        let title: String
        let detail: String
        let at: Date?
        let tone: ConsoleTheme.Tone
    }

    private var recentRecords: [RecentRecord] {
        var items: [RecentRecord] = []
        for row in store.checkin.rows where row.state.isChecked || row.state.isProblem {
            items.append(RecentRecord(
                id: "checkin-\(row.accountId)", title: "签到 · \(row.nickname)",
                detail: row.state.isProblem ? (row.errorDetail ?? row.state.title) : row.state.title,
                at: row.checkedAt, tone: row.state.isProblem ? .warning : .neutral
            ))
        }
        for row in store.travelRows {
            guard let status = row.status, status.claimed || status.stage == .failed else { continue }
            items.append(RecentRecord(
                id: "travel-\(row.accountId)", title: "旅行 · \(row.nickname)",
                detail: status.claimed ? "已领取\(status.rewardCredits.map { " +\($0)" } ?? "")" : (status.lastError ?? "执行失败"),
                at: ConsoleFormat.parse(status.claimedAt) ?? ConsoleFormat.parse(status.lastCheckedAt),
                tone: status.claimed ? .neutral : .warning
            ))
        }
        for item in store.qoderCreditsAuto {
            guard let run = item.lastRun else { continue }
            items.append(RecentRecord(
                id: "credits-\(item.accountId)", title: "Credits · \(item.displayName)",
                detail: run.outcomeLabel + (run.credits.map { " +\($0)" } ?? ""),
                at: ConsoleFormat.parse(run.executedAt), tone: run.isSuccess ? .neutral : .warning
            ))
        }
        return items.sorted { ($0.at ?? .distantPast) > ($1.at ?? .distantPast) }.prefix(5).map { $0 }
    }

    private var recentSection: some View {
        VStack(alignment: .leading, spacing: ConsoleTheme.groupSpacing) {
            ConsoleSectionHeader(title: "最近活动", detail: nil) { EmptyView() }
            if recentRecords.isEmpty {
                Text("完成一次任务后，结果会显示在这里")
                    .font(ConsoleType.body)
                    .foregroundStyle(ConsoleTheme.secondaryText)
            } else {
                ConsoleListSurface {
                    VStack(spacing: 0) {
                        ForEach(Array(recentRecords.enumerated()), id: \.element.id) { index, record in
                            HStack(spacing: 12) {
                                Circle()
                                    .fill(ConsoleTheme.color(record.tone))
                                    .frame(width: 6, height: 6)
                                Text(record.title)
                                    .font(ConsoleType.rowTitle)
                                    .foregroundStyle(ConsoleTheme.text)
                                    .lineLimit(1)
                                    .frame(maxWidth: .infinity, alignment: .leading)
                                Text(record.detail)
                                    .font(ConsoleType.caption)
                                    .foregroundStyle(ConsoleTheme.secondaryText)
                                    .lineLimit(1)
                                    .frame(maxWidth: .infinity, alignment: .leading)
                                Text(record.at.map(ConsoleFormat.shortDateTime) ?? "时间未知")
                                    .font(ConsoleType.caption)
                                    .foregroundStyle(ConsoleTheme.tertiaryText)
                                    .monospacedDigit()
                            }
                            .frame(minHeight: ConsoleTheme.rowHeight)
                            if index < recentRecords.count - 1 { Divider().overlay(ConsoleTheme.border) }
                        }
                    }
                }
            }
        }
    }
}

extension ConsoleFormat {
    static func parse(_ value: String?) -> Date? {
        guard let value, !value.isEmpty else { return nil }
        let parser = ISO8601DateFormatter()
        parser.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        if let date = parser.date(from: value) { return date }
        parser.formatOptions = [.withInternetDateTime]
        return parser.date(from: value)
    }
}
