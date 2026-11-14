import SwiftUI

/// 总览页（PRD §3）。
///
/// 回答四个问题就够：有几个服务账号、授权是否正常、今天完成了多少、有没有要手动处理的。
/// 这里**不**放登录表单、完整额度列表或全部账号——那些属于各自的页面。
struct ConsoleOverviewView: View {
    @ObservedObject var store: ConsoleStore
    @EnvironmentObject private var nav: ConsoleNavigation

    var body: some View {
        // 总览没有"必须的主操作"（PRD §7.1）：刷新走标题栏那颗全局按钮。
        ConsolePageScaffold {
            HStack(spacing: 8) {
                Text(syncSummary)
                    .font(ConsoleType.small)
                    .foregroundStyle(ConsoleTheme.secondaryText)
                    .lineLimit(1)
                Spacer(minLength: 0)
            }
        } content: {
            VStack(alignment: .leading, spacing: ConsoleTheme.sectionSpacing) {
                statSection
                platformSection
                HStack(alignment: .top, spacing: ConsoleTheme.cardSpacing) {
                    recentSection
                    pendingSection.frame(width: 286)
                }
            }
        }
        .task {
            // 只读本地记录：总览打开时不联网，也不改任何业务状态。
            if store.tasksOverview == nil {
                await store.refreshTasksOverview()
            }
            // 签到 / 旅行的"最近记录"来自本地库，读一次才不是空的（不发网络请求）。
            if store.checkin.rows.isEmpty {
                await store.loadCheckin(cached: true)
            }
            if store.travelRows.isEmpty {
                await store.loadTravel()
            }
        }
    }

    private var syncSummary: String {
        var parts: [String] = []
        if let at = store.lastAccountsRefresh {
            parts.append("账号 \(ConsoleFormat.shortDateTime(at))")
        }
        if let at = store.qoderSyncedAt {
            parts.append("Qoder \(ConsoleFormat.shortDateTime(at))")
        }
        if let at = store.tasksOverview?.today {
            parts.append("统计日 \(at)")
        }
        return parts.isEmpty ? "尚未同步" : parts.joined(separator: " · ")
    }

    // MARK: 第一层：状态摘要

    private var statSection: some View {
        ViewThatFits(in: .horizontal) {
            HStack(spacing: ConsoleTheme.cardSpacing) {
                statTiles.frame(minWidth: 186)
            }
            LazyVGrid(
                columns: [GridItem(.flexible(), spacing: ConsoleTheme.cardSpacing),
                          GridItem(.flexible(), spacing: ConsoleTheme.cardSpacing)],
                spacing: ConsoleTheme.cardSpacing
            ) {
                statTiles
            }
        }
    }

    @ViewBuilder
    private var statTiles: some View {
        let fleet = store.fleet
        let overview = store.tasksOverview

        ConsoleStatTile(
            caption: "服务账号",
            value: "\(fleet.serviceAccounts)",
            hint: "WorkBuddy \(fleet.cards[0].accounts + fleet.cards[1].accounts) · Qoder \(fleet.cards[2].accounts + fleet.cards[3].accounts)"
        )
        ConsoleStatTile(
            caption: "授权正常",
            value: "\(fleet.healthy)",
            hint: fleet.issues > 0 ? "\(fleet.issues) 个需要处理" : "没有异常授权",
            tone: fleet.issues > 0 ? ConsoleTheme.warning : ConsoleTheme.text
        )
        ConsoleStatTile(
            caption: "今日完成",
            value: overview.map { "\($0.doneToday)" } ?? "—",
            hint: overview.map { "待完成 \($0.pendingToday)" } ?? "读取本地记录中",
            emphasized: overview != nil
        )
        // 口径必须与左边那张卡一致：授权异常来自账号本身，失败任务来自本地记录，
        // 两者不重叠，可以相加（needsReauth 就是账号级异常，不重复计）。
        ConsoleStatTile(
            caption: "需要处理",
            value: overview.map { "\(fleet.issues + $0.failureCount)" } ?? "\(fleet.issues)",
            hint: "授权 \(fleet.issues) · 失败任务 \(overview?.failureCount ?? 0)",
            tone: fleet.issues > 0 || (overview?.failureCount ?? 0) > 0
                ? ConsoleTheme.danger : ConsoleTheme.text,
            emphasized: true
        )
    }

    // MARK: 第二层：服务平台

    private var platformSection: some View {
        VStack(alignment: .leading, spacing: 8) {
            ConsoleSectionHeader(
                title: "服务平台",
                detail: store.fleet.localProfiles > 0
                    ? "另有 \(store.fleet.localProfiles) 个本机 Qoder 配置，不计入服务账号"
                    : nil
            ) { EmptyView() }

            LazyVGrid(
                columns: [GridItem(.flexible(), spacing: ConsoleTheme.cardSpacing),
                          GridItem(.flexible(), spacing: ConsoleTheme.cardSpacing)],
                spacing: ConsoleTheme.cardSpacing
            ) {
                ForEach(store.fleet.cards) { card in
                    platformCard(card)
                }
            }
        }
    }

    private func platformCard(_ card: ConsolePlatformCard) -> some View {
        Button {
            nav.openAccounts(filter: card.kind.filter)
        } label: {
            ConsoleCard(padding: 12) {
                VStack(alignment: .leading, spacing: 8) {
                    HStack(spacing: 8) {
                        Image(systemName: card.kind.symbol)
                            .font(.system(size: 11, weight: .semibold))
                            .foregroundStyle(ConsoleTheme.accent)
                            .frame(width: 24, height: 24)
                            .background(ConsoleTheme.accentSoft, in: RoundedRectangle(cornerRadius: 7))
                        VStack(alignment: .leading, spacing: 1) {
                            Text(card.kind.title)
                                .font(ConsoleType.rowTitle)
                                .foregroundStyle(ConsoleTheme.text)
                                .lineLimit(1)
                            Text(card.kind.subtitle)
                                .font(ConsoleType.caption)
                                .foregroundStyle(ConsoleTheme.tertiaryText)
                                .lineLimit(1)
                        }
                        Spacer(minLength: 4)
                        Text("\(card.accounts)")
                            .font(ConsoleType.metricSmall)
                            .monospacedDigit()
                            .foregroundStyle(ConsoleTheme.text)
                        Image(systemName: "chevron.right")
                            .font(.system(size: 9, weight: .semibold))
                            .foregroundStyle(ConsoleTheme.tertiaryText)
                    }

                    HStack(spacing: 10) {
                        ConsoleStatusDot(color: ConsoleTheme.success, text: "正常 \(card.healthy)")
                        if card.issues > 0 {
                            ConsoleStatusDot(color: ConsoleTheme.warning, text: "异常 \(card.issues)")
                        } else if card.accounts == 0 {
                            ConsoleStatusDot(color: ConsoleTheme.tertiaryText, text: "未添加账号")
                        } else {
                            ConsoleStatusDot(color: ConsoleTheme.tertiaryText, text: "无异常")
                        }
                        Spacer(minLength: 0)
                    }

                    Text(lastSyncText(for: card.kind))
                        .font(ConsoleType.caption)
                        .foregroundStyle(ConsoleTheme.tertiaryText)
                        .lineLimit(1)
                }
            }
        }
        .buttonStyle(.plain)
        .help("在 AI 账号页筛选 \(card.kind.title)")
    }

    /// 最近同步状态：只写真实拿到的时间，没同步过就直说。
    private func lastSyncText(for kind: ConsolePlatformKind) -> String {
        switch kind {
        case .workbuddyCN, .workbuddyAI:
            if let at = store.lastAccountsRefresh {
                return "最近同步 \(ConsoleFormat.shortDateTime(at))"
            }
            if let error = store.accountsError { return error }
            return "尚未同步"
        case .qoderCN, .qoderGlobal:
            if let at = store.qoderSyncedAt {
                return "最近同步 \(ConsoleFormat.shortDateTime(at))"
            }
            if let error = store.qoderError { return error }
            return "尚未同步"
        }
    }

    // MARK: 第三层：最近记录

    private struct RecentRecord: Identifiable {
        let id: String
        let title: String
        let detail: String
        let at: Date?
        let tone: ConsoleTheme.Tone
        let symbol: String
    }

    private var recentRecords: [RecentRecord] {
        var items: [RecentRecord] = []

        for row in store.checkin.rows where row.state.isChecked || row.state.isProblem {
            let credits = row.credits.map { " +\($0)" } ?? ""
            items.append(RecentRecord(
                id: "checkin-\(row.accountId)",
                title: "签到 · \(row.nickname)",
                detail: row.state.isProblem
                    ? (row.errorDetail ?? row.state.title)
                    : "\(row.state.title)\(credits)",
                at: row.checkedAt,
                tone: row.state.isProblem ? .warning : .ok,
                symbol: "calendar.badge.checkmark"
            ))
        }

        for row in store.travelRows {
            guard let status = row.status, status.claimed || status.stage == .failed else { continue }
            items.append(RecentRecord(
                id: "travel-\(row.accountId)",
                title: "Buddy 旅行 · \(row.nickname)",
                detail: status.claimed
                    ? "已领取\(status.rewardCredits.map { " +\($0)" } ?? "")"
                    : (status.lastError ?? "执行失败"),
                at: ConsoleFormat.parse(status.claimedAt) ?? ConsoleFormat.parse(status.lastCheckedAt),
                tone: status.claimed ? .ok : .warning,
                symbol: "airplane"
            ))
        }

        for item in store.qoderCreditsAuto {
            guard let run = item.lastRun else { continue }
            items.append(RecentRecord(
                id: "credits-\(item.accountId)",
                title: "Qoder Credits · \(item.displayName)",
                detail: run.outcomeLabel + (run.credits.map { " +\($0)" } ?? ""),
                at: ConsoleFormat.parse(run.executedAt),
                tone: run.isSuccess ? .ok : .warning,
                symbol: "circle.dotted.circle"
            ))
        }

        if let at = store.lastGrowthScan, !store.growthResult.scans.isEmpty {
            let pending = store.growthResult.scans.reduce(0) { $0 + $1.pendingCount }
            items.append(RecentRecord(
                id: "growth-scan",
                title: "成长中心扫描",
                detail: "\(store.growthResult.scans.count) 个账号 · 待办 \(pending)",
                at: at,
                tone: .ok,
                symbol: "sparkles"
            ))
        }

        return items
            .sorted { ($0.at ?? .distantPast) > ($1.at ?? .distantPast) }
            .prefix(6)
            .map { $0 }
    }

    private var recentSection: some View {
        VStack(alignment: .leading, spacing: 8) {
            ConsoleSectionHeader(title: "最近记录", detail: nil) { EmptyView() }
            ConsoleCard(padding: 0) {
                let records = recentRecords
                if records.isEmpty {
                    ConsoleEmptyState(
                        symbol: "clock",
                        title: "暂无执行记录",
                        message: "签到、成长或旅行执行过一次后，这里会显示最近结果。"
                    )
                } else {
                    VStack(spacing: 0) {
                        ForEach(Array(records.enumerated()), id: \.element.id) { index, record in
                            recordRow(record)
                            if index < records.count - 1 {
                                Divider().overlay(ConsoleTheme.border).padding(.leading, 38)
                            }
                        }
                    }
                }
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private func recordRow(_ record: RecentRecord) -> some View {
        HStack(spacing: 10) {
            Image(systemName: record.symbol)
                .font(.system(size: 11))
                .foregroundStyle(ConsoleTheme.color(record.tone))
                .frame(width: 18)
            VStack(alignment: .leading, spacing: 1) {
                Text(record.title)
                    .font(ConsoleType.rowTitle)
                    .foregroundStyle(ConsoleTheme.text)
                    .lineLimit(1)
                Text(record.detail)
                    .font(ConsoleType.caption)
                    .foregroundStyle(ConsoleTheme.secondaryText)
                    .lineLimit(1)
            }
            Spacer(minLength: 6)
            Text(record.at.map(ConsoleFormat.shortDateTime) ?? "时间未知")
                .font(ConsoleType.caption)
                .foregroundStyle(ConsoleTheme.tertiaryText)
                .monospacedDigit()
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
    }

    // MARK: 待处理事项

    private struct PendingItem: Identifiable {
        let id: String
        let title: String
        let detail: String
        let selection: ConsoleAccountSelection?
        /// 跳转时要应用到的平台筛选：否则账号会被当前筛选挡住。
        let filter: ConsolePlatformFilter
        let destination: ConsolePage
    }

    private var pendingItems: [PendingItem] {
        var items: [PendingItem] = []

        for account in store.accounts where ["expired", "invalid", "denied"].contains(account.authStatus) {
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

        if let overview = store.tasksOverview {
            for failure in overview.recentFailures.prefix(3) {
                items.append(PendingItem(
                    id: "fail-\(failure.id)",
                    title: failure.displayName,
                    detail: "\(failure.taskLabel) · \(failure.detail ?? failure.status)",
                    selection: nil,
                    filter: .all,
                    destination: .checkin
                ))
            }
        }

        if let error = store.accountsError {
            items.append(PendingItem(
                id: "accounts-error",
                title: "账号读取失败",
                detail: error,
                selection: nil,
                filter: .all,
                destination: .accounts
            ))
        }
        if let error = store.qoderError {
            items.append(PendingItem(
                id: "qoder-error",
                title: "Qoder 读取失败",
                detail: error,
                selection: nil,
                filter: .all,
                destination: .accounts
            ))
        }

        return Array(items.prefix(5))
    }

    private var pendingSection: some View {
        VStack(alignment: .leading, spacing: 8) {
            ConsoleSectionHeader(title: "待处理", detail: nil) { EmptyView() }
            ConsoleCard(padding: 0) {
                let items = pendingItems
                if items.isEmpty {
                    ConsoleEmptyState(
                        symbol: "checkmark.circle",
                        title: "没有需要处理的事项",
                        message: "授权失效、任务失败会出现在这里。"
                    )
                } else {
                    VStack(spacing: 0) {
                        ForEach(items) { item in
                            Button {
                                if item.destination == .accounts {
                                    nav.openAccounts(filter: item.filter, selection: item.selection)
                                } else {
                                    nav.openCheckin(platform: .workbuddy)
                                }
                            } label: {
                                HStack(spacing: 8) {
                                    Circle()
                                        .fill(ConsoleTheme.warning)
                                        .frame(width: 6, height: 6)
                                    VStack(alignment: .leading, spacing: 1) {
                                        Text(item.title)
                                            .font(ConsoleType.rowTitle)
                                            .foregroundStyle(ConsoleTheme.text)
                                            .lineLimit(1)
                                        Text(item.detail)
                                            .font(ConsoleType.caption)
                                            .foregroundStyle(ConsoleTheme.secondaryText)
                                            .lineLimit(2)
                                    }
                                    Spacer(minLength: 4)
                                    Image(systemName: "chevron.right")
                                        .font(.system(size: 9, weight: .semibold))
                                        .foregroundStyle(ConsoleTheme.tertiaryText)
                                }
                                .padding(.horizontal, 12)
                                .padding(.vertical, 8)
                                .contentShape(Rectangle())
                            }
                            .buttonStyle(.plain)
                        }
                    }
                }
            }
        }
    }
}

extension ConsoleFormat {
    /// RFC3339 → Date。解析不了返回 nil，不猜时间。
    static func parse(_ value: String?) -> Date? {
        guard let value, !value.isEmpty else { return nil }
        let parser = ISO8601DateFormatter()
        parser.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        if let date = parser.date(from: value) { return date }
        parser.formatOptions = [.withInternetDateTime]
        return parser.date(from: value)
    }
}
