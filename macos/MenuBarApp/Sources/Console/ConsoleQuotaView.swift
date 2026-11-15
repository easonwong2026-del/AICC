import SwiftUI

/// 额度看板（PRD §5）：顶部切换来源，两套数据不混排、不相加。
///
/// - **Magpie 额度**：继续用 `WidgetStatusLoader` + `QuotaSnapshot.slots`，一行代码都没换；
/// - **账号额度**：`quota.board` 的账号级余额与用量。
///
/// 这里**只读**：不发签到、不发领取。领取统一在签到任务页。
struct ConsoleQuotaView: View {
    @ObservedObject var store: ConsoleStore
    @EnvironmentObject private var nav: ConsoleNavigation

    @State private var providerQuery = ""

    var body: some View {
        ConsolePageScaffold {
            toolbar
        } content: {
            VStack(alignment: .leading, spacing: ConsoleTheme.sectionSpacing) {
                switch nav.quotaSource {
                case .magpie:
                    magpieContent
                case .accounts:
                    accountContent
                }
            }
        }
    }

    // MARK: - 工具栏

    private var toolbar: some View {
        HStack(spacing: 8) {
            Picker("来源", selection: $nav.quotaSource) {
                ForEach(ConsoleQuotaSource.allCases) { source in
                    Text(source.title).tag(source)
                }
            }
            .pickerStyle(.segmented)
            .labelsHidden()
            .tint(ConsoleTheme.accent)
            .frame(width: 220)

            Spacer(minLength: 8)

            switch nav.quotaSource {
            case .magpie:
                if !providerOptions.isEmpty {
                    ConsoleFilterMenu(
                        title: "服务商筛选",
                        options: providerOptions,
                        label: { $0.title },
                        selection: Binding(
                            get: {
                                providerOptions.first { $0.id == nav.quotaProvider }
                                    ?? providerOptions[0]
                            },
                            set: { nav.quotaProvider = $0.id }
                        )
                    )
                }
                if store.loadingQuota {
                    ProgressView().controlSize(.small)
                }
                Button("刷新额度") {
                    Task { await store.refreshQuota(force: true) }
                }
                .buttonStyle(ConsolePrimaryButtonStyle())
                .disabled(store.loadingQuota)
            case .accounts:
                Picker("窗口", selection: Binding(
                    get: { store.workerQuotaWindowDays },
                    set: { days in Task { await store.refreshWorkerQuota(force: false, days: days) } }
                )) {
                    Text("7 天").tag(7)
                    Text("30 天").tag(30)
                }
                .pickerStyle(.segmented)
                .labelsHidden()
                .tint(ConsoleTheme.accent)
                .frame(width: 130)

                if store.loadingWorkerQuota {
                    ProgressView().controlSize(.small)
                }
                Button("刷新额度") {
                    Task { await store.refreshWorkerQuota(force: true) }
                }
                .buttonStyle(ConsolePrimaryButtonStyle())
                .disabled(store.loadingWorkerQuota)
            }
        }
    }

    // MARK: - Magpie 额度

    private struct ProviderOption: Identifiable, Equatable {
        let id: String
        let title: String
    }

    private var providerOptions: [ProviderOption] {
        guard let snapshot = store.snapshot else { return [] }
        var seen: Set<String> = []
        var options = [ProviderOption(id: "", title: "全部服务商")]
        for provider in snapshot.providers where !seen.contains(provider) {
            seen.insert(provider)
            options.append(ProviderOption(
                id: provider,
                title: MagpieQuotaParser.providerName(provider)
            ))
        }
        return options
    }

    @ViewBuilder
    private var magpieContent: some View {
        dataStatusRow

        if let snapshot = store.snapshot, !snapshot.accounts.isEmpty {
            providerSections(snapshot)
        } else {
            ConsoleCard {
                ConsoleEmptyState(
                    symbol: "chart.bar.doc.horizontal",
                    title: "没有读到额度",
                    message: store.quotaError ?? "启动 Magpie 后这里会显示各服务商的额度窗口。",
                    actionTitle: "重新读取",
                    action: { Task { await store.refreshQuota(force: true) } }
                )
            }
        }
    }

    private var dataStatusRow: some View {
        HStack(spacing: 8) {
            if let snapshot = store.snapshot {
                ConsoleBadge(
                    text: snapshot.stale ? "缓存" : "实时",
                    tint: snapshot.stale ? ConsoleTheme.warning : ConsoleTheme.success
                )
                Text("\(snapshot.providers.count) 个服务商 · \(snapshot.accounts.count) 张额度卡")
                    .font(ConsoleType.small)
                    .foregroundStyle(ConsoleTheme.secondaryText)
                if let updated = snapshot.updatedAt {
                    Text("更新于 \(updated)")
                        .font(ConsoleType.caption)
                        .foregroundStyle(ConsoleTheme.tertiaryText)
                }
            } else {
                Text("尚未读取 Magpie 额度")
                    .font(ConsoleType.small)
                    .foregroundStyle(ConsoleTheme.secondaryText)
            }
            Spacer(minLength: 0)
        }
    }

    private struct ProviderGroup {
        let provider: String
        let name: String
        let cards: [QuotaCard]
    }

    private func groupedByProvider(_ snapshot: QuotaSnapshot) -> [ProviderGroup] {
        var order: [String] = []
        var buckets: [String: [QuotaCard]] = [:]
        for card in snapshot.accounts {
            if buckets[card.provider] == nil {
                order.append(card.provider)
                buckets[card.provider] = []
            }
            buckets[card.provider]?.append(card)
        }
        return order
            .filter { nav.quotaProvider.isEmpty || $0 == nav.quotaProvider }
            .map { provider in
                let cards = buckets[provider] ?? []
                return ProviderGroup(
                    provider: provider,
                    name: cards.first?.providerName ?? MagpieQuotaParser.providerName(provider),
                    cards: cards
                )
            }
    }

    private func providerSections(_ snapshot: QuotaSnapshot) -> some View {
        let groups = groupedByProvider(snapshot)
        return VStack(alignment: .leading, spacing: ConsoleTheme.sectionSpacing) {
            ForEach(groups, id: \.provider) { group in
                VStack(alignment: .leading, spacing: 8) {
                    ConsoleSectionHeader(title: group.name, detail: "\(group.cards.count) 个账号") {
                        EmptyView()
                    }
                    LazyVGrid(
                        columns: [GridItem(.adaptive(minimum: 300), spacing: ConsoleTheme.cardSpacing)],
                        spacing: ConsoleTheme.cardSpacing
                    ) {
                        ForEach(group.cards) { card in
                            quotaCard(card)
                        }
                    }
                }
            }
        }
    }

    private func quotaCard(_ card: QuotaCard) -> some View {
        ConsoleCard(padding: 12) {
            VStack(alignment: .leading, spacing: 10) {
                HStack(alignment: .firstTextBaseline, spacing: 6) {
                    Text(card.accountLabel)
                        .font(ConsoleType.rowTitle)
                        .foregroundStyle(ConsoleTheme.text)
                        .lineLimit(1)
                    if let plan = card.plan, !plan.isEmpty {
                        ConsoleBadge(text: plan, tint: ConsoleTheme.secondaryText)
                    }
                    Spacer(minLength: 0)
                    if card.stale {
                        ConsoleBadge(text: "缓存", tint: ConsoleTheme.warning)
                    }
                }

                if card.isBalance {
                    balanceRows(card)
                } else if card.items.isEmpty {
                    Text("该账号没有可显示的额度窗口")
                        .font(ConsoleType.small)
                        .foregroundStyle(ConsoleTheme.secondaryText)
                } else {
                    VStack(spacing: 9) {
                        ForEach(Array(card.items.enumerated()), id: \.offset) { _, item in
                            itemRow(item)
                        }
                    }
                }

                if let status = card.statusText, !status.isEmpty {
                    Text(status)
                        .font(ConsoleType.caption)
                        .foregroundStyle(ConsoleTheme.secondaryText)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
        }
    }

    private func itemRow(_ item: QuotaItem) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Text(item.name)
                    .font(ConsoleType.small)
                    .foregroundStyle(ConsoleTheme.secondaryText)
                    .lineLimit(1)
                Spacer(minLength: 6)
                Text(item.primaryText)
                    .font(ConsoleType.body)
                    .monospacedDigit()
                    .foregroundStyle(ConsoleTheme.text)
            }
            if let share = item.remaining, item.isPercentage {
                ConsoleShareBar(share: share, tone: barTone(share))
            }
            HStack(spacing: 8) {
                if let total = item.totalText {
                    Text(total)
                        .font(ConsoleType.caption)
                        .foregroundStyle(ConsoleTheme.tertiaryText)
                }
                if let reset = item.reset, !reset.isEmpty {
                    Text("重置于 \(reset)")
                        .font(ConsoleType.caption)
                        .foregroundStyle(ConsoleTheme.tertiaryText)
                }
                Spacer(minLength: 0)
            }
        }
    }

    private func balanceRows(_ card: QuotaCard) -> some View {
        VStack(alignment: .leading, spacing: 5) {
            Text(card.balanceText)
                .font(ConsoleType.metricSmall)
                .monospacedDigit()
                .foregroundStyle(ConsoleTheme.text)
            ForEach(Array(card.balanceParts.enumerated()), id: \.offset) { _, part in
                HStack(spacing: 6) {
                    Text(part.label ?? "余额")
                        .font(ConsoleType.caption)
                        .foregroundStyle(ConsoleTheme.secondaryText)
                    Text(part.text)
                        .font(ConsoleType.caption)
                        .foregroundStyle(ConsoleTheme.tertiaryText)
                    Spacer(minLength: 0)
                }
            }
        }
    }

    private func barTone(_ share: Double) -> Color {
        if share <= 10 { return ConsoleTheme.danger }
        if share <= 30 { return ConsoleTheme.warning }
        return ConsoleTheme.accent
    }

    // MARK: - 账号额度（官方接口）

    @ViewBuilder
    private var accountContent: some View {
        HStack(spacing: 8) {
            Text("账号级余额与用量。每日奖励不是余额，不与余额相加。")
                .font(ConsoleType.small)
                .foregroundStyle(ConsoleTheme.secondaryText)
            Spacer(minLength: 0)
        }

        if let error = store.workerQuotaError, store.workerQuotaRows.isEmpty {
            ConsoleCard {
                ConsoleEmptyState(
                    symbol: "gauge.with.dots.needle.33percent",
                    title: "没有读到账号额度",
                    message: error,
                    actionTitle: "重新读取",
                    action: { Task { await store.refreshWorkerQuota(force: true) } }
                )
            }
        } else if store.workerQuotaRows.isEmpty {
            ConsoleCard {
                ConsoleLoadingRow(text: "正在向官方接口读取账号额度…")
            }
        } else {
            ForEach(platformGroups, id: \.name) { group in
                VStack(alignment: .leading, spacing: 8) {
                    ConsoleSectionHeader(title: group.name, detail: "\(group.rows.count) 个账号") {
                        EmptyView()
                    }
                    LazyVGrid(
                        columns: [GridItem(.adaptive(minimum: 300), spacing: ConsoleTheme.cardSpacing)],
                        spacing: ConsoleTheme.cardSpacing
                    ) {
                        ForEach(group.rows) { row in
                            accountQuotaCard(row)
                        }
                    }
                }
            }
        }
    }

    private struct PlatformGroup {
        let name: String
        let rows: [WorkerQuotaRow]
    }

    private var platformGroups: [PlatformGroup] {
        var order: [String] = []
        var buckets: [String: [WorkerQuotaRow]] = [:]
        for row in store.workerQuotaRows {
            let name = row.providerName
            if buckets[name] == nil {
                order.append(name)
                buckets[name] = []
            }
            buckets[name]?.append(row)
        }
        return order.map { PlatformGroup(name: $0, rows: buckets[$0] ?? []) }
    }

    private func accountQuotaCard(_ row: WorkerQuotaRow) -> some View {
        ConsoleCard(padding: 12) {
            VStack(alignment: .leading, spacing: 8) {
                HStack(alignment: .firstTextBaseline, spacing: 6) {
                    Text(row.nickname.isEmpty ? row.accountId : row.nickname)
                        .font(ConsoleType.rowTitle)
                        .foregroundStyle(ConsoleTheme.text)
                        .lineLimit(1)
                    if let plan = row.plan, !plan.isEmpty {
                        ConsoleBadge(text: plan, tint: ConsoleTheme.secondaryText)
                    }
                    Spacer(minLength: 0)
                    if row.stale {
                        ConsoleBadge(text: "过期", tint: ConsoleTheme.danger)
                    } else if row.cached {
                        ConsoleBadge(text: "缓存", tint: ConsoleTheme.warning)
                    }
                }

                if let credits = row.credits {
                    creditsReadOnlyBlock(credits, row: row)
                } else {
                    quotaBody(row)
                }

                HStack(spacing: 6) {
                    Text(row.sourceName)
                    if let capturedAt = row.capturedAt, !capturedAt.isEmpty {
                        Text("· 数据于 \(ConsoleQuotaView.localTimestamp(capturedAt))")
                    }
                    Spacer(minLength: 0)
                }
                .font(ConsoleType.caption)
                .foregroundStyle(ConsoleTheme.tertiaryText)

                if let error = row.error, !error.isEmpty {
                    Text(error)
                        .font(ConsoleType.caption)
                        .foregroundStyle(ConsoleTheme.tertiaryText)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
        }
    }

    @ViewBuilder
    private func quotaBody(_ row: WorkerQuotaRow) -> some View {
        switch row.status {
        case "ok", "stale":
            if let text = row.remainingText {
                HStack(alignment: .firstTextBaseline, spacing: 6) {
                    Text(text)
                        .font(ConsoleType.metricSmall)
                        .monospacedDigit()
                        .foregroundStyle(ConsoleTheme.text)
                    if let unit = row.unit {
                        Text(unit)
                            .font(ConsoleType.caption)
                            .foregroundStyle(ConsoleTheme.tertiaryText)
                    }
                    Spacer(minLength: 0)
                }
                if let share = row.remainingShare {
                    ConsoleShareBar(share: share, tone: share <= 30 ? ConsoleTheme.warning : ConsoleTheme.accent)
                }
                if let total = row.totalText {
                    Text(total)
                        .font(ConsoleType.caption)
                        .foregroundStyle(ConsoleTheme.tertiaryText)
                }
            } else {
                // 拿到 200 但没拿到额度：显示未知，不显示 0。
                Text("额度未知")
                    .font(ConsoleType.body)
                    .foregroundStyle(ConsoleTheme.secondaryText)
            }
            if let usage = row.usage, usage.requests > 0 {
                Text("近 \(usage.windowDays) 天 \(usage.requests) 次请求 · 消耗 "
                    + "\(WorkerQuotaRow.format(usage.credits))\(usage.truncated ? "（已截断）" : "")")
                    .font(ConsoleType.caption)
                    .foregroundStyle(ConsoleTheme.tertiaryText)
            } else if let usageError = row.usageError, !usageError.isEmpty {
                Text("用量读取失败：\(usageError)")
                    .font(ConsoleType.caption)
                    .foregroundStyle(ConsoleTheme.warning)
            }
        case "unverified":
            Text("未验证（该平台没有已证实的接口，未发送请求）")
                .font(ConsoleType.small)
                .foregroundStyle(ConsoleTheme.secondaryText)
        case "unauthorized":
            Text("授权不可用，需要重新登录")
                .font(ConsoleType.small)
                .foregroundStyle(ConsoleTheme.danger)
        default:
            Text("读取失败")
                .font(ConsoleType.small)
                .foregroundStyle(ConsoleTheme.danger)
        }
    }

    /// Qoder 卡片里的 Credits：**活动状态，不是余额**。
    ///
    /// 额度页只做看和跳转：领取按钮在签到任务页（PRD §5.4）。
    private func creditsReadOnlyBlock(_ credits: QoderCreditsInfo, row: WorkerQuotaRow) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(alignment: .firstTextBaseline, spacing: 6) {
                Text(credits.summary)
                    .font(ConsoleType.body)
                    .foregroundStyle(credits.state == "error" || credits.state == "unauthorized"
                        ? ConsoleTheme.warning : ConsoleTheme.text)
                    .fixedSize(horizontal: false, vertical: true)
                Spacer(minLength: 0)
                ConsoleBadge(text: "每日奖励", tint: ConsoleTheme.secondaryText)
            }
            if let end = credits.windowEnd {
                Text("活动窗口截止 \(Self.localTimestamp(Self.epochToRFC3339(end)))（服务端时间）")
                    .font(ConsoleType.caption)
                    .foregroundStyle(ConsoleTheme.tertiaryText)
            }
            HStack(spacing: 8) {
                Button("到签到任务查看") {
                    nav.openCheckin(platform: .qoder)
                }
                .buttonStyle(ConsoleSecondaryButtonStyle())
                Text("领取操作只在签到任务页执行")
                    .font(ConsoleType.caption)
                    .foregroundStyle(ConsoleTheme.tertiaryText)
                Spacer(minLength: 0)
            }
        }
    }

    /// Unix 秒 → RFC3339，交给 `localTimestamp` 统一按本机时区显示。
    static func epochToRFC3339(_ seconds: Int) -> String {
        let date = Date(timeIntervalSince1970: TimeInterval(seconds))
        let formatter = ISO8601DateFormatter()
        formatter.formatOptions = [.withInternetDateTime]
        return formatter.string(from: date)
    }

    /// RFC3339（UTC）→ 本机时区的 `MM-dd HH:mm`。
    static func localTimestamp(_ raw: String) -> String {
        guard let date = ConsoleFormat.parse(raw) else {
            return String(raw.prefix(16)).replacingOccurrences(of: "T", with: " ")
        }
        return ConsoleFormat.shortDateTime(date)
    }
}
