import SwiftUI

/// 额度看板：Magpie 网关额度与服务账号官方额度是两条互不相加的链路。
/// 这里只读；每日奖励的领取在签到任务页。
struct ConsoleQuotaView: View {
    @ObservedObject var store: ConsoleStore
    @EnvironmentObject private var nav: ConsoleNavigation

    private enum Column {
        static let accountMin: CGFloat = 140
        static let magpieWindow: CGFloat = 96
        static let magpieRemaining: CGFloat = 116
        static let magpieUsed: CGFloat = 88
        static let magpieReset: CGFloat = 96
        static let accountRemaining: CGFloat = 128
        static let accountUsage: CGFloat = 166
        static let accountUpdated: CGFloat = 92
        static let status: CGFloat = 72
        static let spacing: CGFloat = 8
        static let inset: CGFloat = 16
    }

    var body: some View {
        ConsolePageScaffold {
            toolbar
        } content: {
            VStack(alignment: .leading, spacing: ConsoleTheme.sectionSpacing) {
                summaryRow
                switch nav.quotaSource {
                case .magpie: magpieContent
                case .accounts: accountContent
                }
            }
        }
    }

    // MARK: - 工具栏

    private var toolbar: some View {
        HStack(spacing: 10) {
            Picker("来源", selection: $nav.quotaSource) {
                ForEach(ConsoleQuotaSource.allCases) { source in
                    Text(source.title).tag(source)
                }
            }
            .pickerStyle(.segmented)
            .labelsHidden()
            .tint(ConsoleTheme.accent)
            .frame(width: 236)

            Spacer(minLength: 8)

            switch nav.quotaSource {
            case .magpie:
                if magpieProviderOptions.count > 1 {
                    providerFilter(options: magpieProviderOptions, title: "服务商")
                }
                if store.loadingQuota { ProgressView().controlSize(.small) }
                Button("刷新") { Task { await store.refreshQuota(force: true) } }
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
                .frame(width: 120)

                if accountProviderOptions.count > 1 {
                    providerFilter(options: accountProviderOptions, title: "平台")
                }
                if store.loadingWorkerQuota { ProgressView().controlSize(.small) }
                Button("刷新") { Task { await store.refreshWorkerQuota(force: true) } }
                    .buttonStyle(ConsolePrimaryButtonStyle())
                    .disabled(store.loadingWorkerQuota)
            }
        }
    }

    private struct ProviderOption: Identifiable, Equatable {
        let id: String
        let title: String
    }

    private var magpieProviderOptions: [ProviderOption] {
        guard let snapshot = store.snapshot else { return [] }
        var options = [ProviderOption(id: "", title: "全部服务商")]
        var seen: Set<String> = []
        for provider in snapshot.providers where !seen.contains(provider) {
            seen.insert(provider)
            options.append(ProviderOption(id: provider, title: MagpieQuotaParser.providerName(provider)))
        }
        return options
    }

    private var accountProviderOptions: [ProviderOption] {
        var options = [ProviderOption(id: "", title: "全部平台")]
        var seen: Set<String> = []
        for row in store.workerQuotaRows where !seen.contains(row.provider) {
            seen.insert(row.provider)
            options.append(ProviderOption(id: row.provider, title: row.providerName))
        }
        return options
    }

    /// 导航可能带着另一个来源的 provider（例如从账号详情跳来）；不匹配时按“全部”显示，
    /// 但不覆写用户状态。
    private func effectiveProvider(_ options: [ProviderOption]) -> String {
        options.contains(where: { $0.id == nav.quotaProvider }) ? nav.quotaProvider : ""
    }

    private func providerFilter(options: [ProviderOption], title: String) -> some View {
        let effective = effectiveProvider(options)
        return ConsoleFilterMenu(
            title: title,
            options: options,
            label: { $0.title },
            selection: Binding(
                get: { options.first { $0.id == effective } ?? options[0] },
                set: { nav.quotaProvider = $0.id }
            )
        )
    }

    // MARK: - 摘要

    @ViewBuilder
    private var summaryRow: some View {
        switch nav.quotaSource {
        case .magpie:
            // 没有数据时由正文的加载/空/错误状态说明，摘要不再重复一遍。
            if let snapshot = store.snapshot, !snapshot.accounts.isEmpty {
                HStack(spacing: 8) {
                    Text("\(filteredMagpieCards.count) 个账号 · \(filteredGroups.count) 个服务商")
                        .font(ConsoleType.small)
                        .foregroundStyle(ConsoleTheme.secondaryText)
                    if snapshot.stale {
                        ConsoleBadge(text: "缓存", tint: ConsoleTheme.warning)
                    }
                    if let updated = snapshot.updatedAt, !updated.isEmpty {
                        Text("更新于 \(updated)")
                            .font(ConsoleType.micro)
                            .foregroundStyle(ConsoleTheme.tertiaryText)
                    }
                    Spacer(minLength: 0)
                }
            }
        case .accounts:
            if !filteredAccountRows.isEmpty {
                HStack(spacing: 8) {
                    Text("\(filteredAccountRows.count) 个账号 · 近 \(store.workerQuotaWindowDays) 天用量")
                        .font(ConsoleType.small)
                        .foregroundStyle(ConsoleTheme.secondaryText)
                    let staleCount = filteredAccountRows.filter {
                        QuotaDisplayState.worker($0) == .stale || QuotaDisplayState.worker($0) == .cached
                    }.count
                    if staleCount > 0 {
                        ConsoleBadge(text: "旧快照 \(staleCount)", tint: ConsoleTheme.warning)
                    }
                    Spacer(minLength: 0)
                }
            }
        }
    }

    // MARK: - Magpie 额度

    private struct ProviderGroup {
        let provider: String
        let name: String
        let cards: [QuotaCard]
    }

    private var filteredMagpieCards: [QuotaCard] {
        guard let snapshot = store.snapshot else { return [] }
        let provider = effectiveProvider(magpieProviderOptions)
        guard !provider.isEmpty else { return snapshot.accounts }
        return snapshot.accounts.filter { $0.provider == provider }
    }

    private var filteredGroups: [ProviderGroup] {
        var order: [String] = []
        var buckets: [String: [QuotaCard]] = [:]
        for card in filteredMagpieCards {
            if buckets[card.provider] == nil {
                order.append(card.provider)
                buckets[card.provider] = []
            }
            buckets[card.provider]?.append(card)
        }
        return order.map { provider in
            let cards = buckets[provider] ?? []
            return ProviderGroup(
                provider: provider,
                name: cards.first?.providerName ?? MagpieQuotaParser.providerName(provider),
                cards: cards
            )
        }
    }

    @ViewBuilder
    private var magpieContent: some View {
        if store.loadingQuota && store.snapshot == nil {
            ConsoleLoadingRow(text: "正在读取 Magpie 额度")
        } else if store.snapshot != nil, !filteredMagpieCards.isEmpty {
            ForEach(filteredGroups, id: \.provider) { group in
                magpieTable(group)
            }
        } else if let error = store.quotaError {
            errorState(title: "没有读到 Magpie 额度", message: error) {
                Task { await store.refreshQuota(force: true) }
            }
        } else if store.snapshot != nil, !magpieProviderOptions.isEmpty,
                  !effectiveProvider(magpieProviderOptions).isEmpty {
            filteredEmptyState(
                title: "该服务商没有额度",
                message: "当前筛选下没有可显示的服务商额度。",
                actionTitle: "显示全部"
            ) { nav.quotaProvider = "" }
        } else {
            emptyState(
                title: "还没有额度数据",
                message: "刷新后会显示 Magpie 采集到的服务商额度。"
            ) { Task { await store.refreshQuota(force: true) } }
        }
    }

    private func magpieTable(_ group: ProviderGroup) -> some View {
        let rows = group.cards.flatMap {
            MagpieQuotaDisplayRow.rows(for: $0, snapshotStale: store.snapshot?.stale == true)
        }
        return VStack(alignment: .leading, spacing: 8) {
            ConsoleSectionHeader(title: group.name, detail: "\(group.cards.count) 个账号") {
                EmptyView()
            }
            ConsoleListSurface(horizontalPadding: 0) {
                VStack(spacing: 0) {
                    magpieHeader
                    Divider().overlay(ConsoleTheme.border)
                    ForEach(Array(rows.enumerated()), id: \.element.id) { index, row in
                        magpieTableRow(row)
                        if index < rows.count - 1 {
                            Divider().overlay(ConsoleTheme.border).padding(.leading, Column.inset)
                        }
                    }
                }
            }
        }
    }

    private var magpieHeader: some View {
        HStack(spacing: Column.spacing) {
            headerCell("账号", width: nil)
            headerCell("窗口", width: Column.magpieWindow)
            headerCell("剩余额度", width: Column.magpieRemaining)
            headerCell("已用", width: Column.magpieUsed)
            headerCell("重置", width: Column.magpieReset)
            headerCell("状态", width: Column.status)
        }
        .padding(.horizontal, Column.inset)
        .padding(.vertical, 8)
    }

    private func magpieTableRow(_ row: MagpieQuotaDisplayRow) -> some View {
        HStack(spacing: Column.spacing) {
            VStack(alignment: .leading, spacing: 2) {
                Text(row.accountName)
                    .font(row.isFirstRow ? ConsoleType.rowTitle : ConsoleType.small)
                    .foregroundStyle(row.isFirstRow ? ConsoleTheme.text : ConsoleTheme.secondaryText)
                    .lineLimit(1)
                    .truncationMode(.middle)
                    .help(row.accountName)
                if row.isFirstRow, let plan = row.plan, !plan.isEmpty {
                    Text(plan).font(ConsoleType.micro).foregroundStyle(ConsoleTheme.tertiaryText)
                }
            }
            .frame(minWidth: Column.accountMin, maxWidth: .infinity, alignment: .leading)

            Text(row.windowName)
                .font(ConsoleType.small)
                .foregroundStyle(ConsoleTheme.secondaryText)
                .lineLimit(1)
                .minimumScaleFactor(0.8)
                .frame(width: Column.magpieWindow, alignment: .leading)
                .help(row.windowName)

            VStack(alignment: .leading, spacing: 3) {
                HStack(alignment: .firstTextBaseline, spacing: 4) {
                    Text(row.remaining)
                        .font(ConsoleType.rowTitle)
                        .monospacedDigit()
                        .foregroundStyle(row.remaining == "—" ? ConsoleTheme.secondaryText : ConsoleTheme.text)
                        .lineLimit(1)
                        .minimumScaleFactor(0.65)
                    if let unit = row.unit {
                        Text(unit)
                            .font(ConsoleType.micro)
                            .foregroundStyle(ConsoleTheme.tertiaryText)
                            .lineLimit(1)
                            .minimumScaleFactor(0.65)
                    }
                }
                if let share = row.remainingShare {
                    ConsoleShareBar(share: share, tone: shareTone(share))
                        .frame(maxWidth: 88)
                }
                if let detail = row.detail {
                    Text(detail)
                        .font(ConsoleType.micro)
                        .foregroundStyle(ConsoleTheme.tertiaryText)
                        .lineLimit(1)
                        .help(detail)
                }
            }
            .frame(width: Column.magpieRemaining, alignment: .leading)

            Text(row.usedText ?? "—")
                .font(ConsoleType.body)
                .monospacedDigit()
                .foregroundStyle(ConsoleTheme.secondaryText)
                .frame(width: Column.magpieUsed, alignment: .leading)

            Text(row.reset ?? "—")
                .font(ConsoleType.caption)
                .foregroundStyle(ConsoleTheme.tertiaryText)
                .lineLimit(1)
                .minimumScaleFactor(0.8)
                .frame(width: Column.magpieReset, alignment: .leading)

            statusCell(row.state)
                .frame(width: Column.status, alignment: .leading)
        }
        .padding(.horizontal, Column.inset)
        .padding(.vertical, 8)
        .frame(minHeight: ConsoleTheme.rowHeight)
    }

    // MARK: - 账号额度

    private struct PlatformGroup {
        let name: String
        let rows: [WorkerQuotaRow]
    }

    private var filteredAccountRows: [WorkerQuotaRow] {
        let provider = effectiveProvider(accountProviderOptions)
        guard !provider.isEmpty else { return store.workerQuotaRows }
        return store.workerQuotaRows.filter { $0.provider == provider }
    }

    private var platformGroups: [PlatformGroup] {
        var order: [String] = []
        var buckets: [String: [WorkerQuotaRow]] = [:]
        for row in filteredAccountRows {
            let name = row.providerName
            if buckets[name] == nil {
                order.append(name)
                buckets[name] = []
            }
            buckets[name]?.append(row)
        }
        return order.map { PlatformGroup(name: $0, rows: buckets[$0] ?? []) }
    }

    @ViewBuilder
    private var accountContent: some View {
        if store.loadingWorkerQuota && store.workerQuotaRows.isEmpty {
            ConsoleLoadingRow(text: "正在读取账号额度")
        } else if !filteredAccountRows.isEmpty {
            ForEach(platformGroups, id: \.name) { group in
                accountTable(group)
            }
        } else if let error = store.workerQuotaError {
            errorState(title: "没有读到账号额度", message: error) {
                Task { await store.refreshWorkerQuota(force: true) }
            }
        } else if !store.workerQuotaRows.isEmpty,
                  !effectiveProvider(accountProviderOptions).isEmpty {
            filteredEmptyState(
                title: "该平台没有额度",
                message: "当前筛选下没有可显示的账号额度。",
                actionTitle: "显示全部"
            ) { nav.quotaProvider = "" }
        } else {
            emptyState(
                title: "还没有账号额度",
                message: "刷新后会读取账号级余额与用量。"
            ) { Task { await store.refreshWorkerQuota(force: true) } }
        }
    }

    private func accountTable(_ group: PlatformGroup) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            ConsoleSectionHeader(title: group.name, detail: "\(group.rows.count) 个账号") { EmptyView() }
            ConsoleListSurface(horizontalPadding: 0) {
                VStack(spacing: 0) {
                    accountHeader
                    Divider().overlay(ConsoleTheme.border)
                    ForEach(Array(group.rows.enumerated()), id: \.element.id) { index, source in
                        accountTableRow(AccountQuotaDisplay(row: source))
                        if index < group.rows.count - 1 {
                            Divider().overlay(ConsoleTheme.border).padding(.leading, Column.inset)
                        }
                    }
                }
            }
        }
    }

    private var accountHeader: some View {
        HStack(spacing: Column.spacing) {
            headerCell("账号", width: nil)
            headerCell("剩余额度", width: Column.accountRemaining)
            headerCell("用量", width: Column.accountUsage)
            headerCell("更新", width: Column.accountUpdated)
            headerCell("状态", width: Column.status)
        }
        .padding(.horizontal, Column.inset)
        .padding(.vertical, 8)
    }

    private func accountTableRow(_ row: AccountQuotaDisplay) -> some View {
        HStack(spacing: Column.spacing) {
            VStack(alignment: .leading, spacing: 2) {
                Text(row.accountName)
                    .font(ConsoleType.rowTitle)
                    .foregroundStyle(ConsoleTheme.text)
                    .lineLimit(1)
                    .truncationMode(.middle)
                    .help(row.accountName)
                if let plan = row.plan, !plan.isEmpty {
                    Text(plan).font(ConsoleType.micro).foregroundStyle(ConsoleTheme.tertiaryText)
                }
                if let credits = row.creditsBadge {
                    Button {
                        nav.openCheckin(platform: .qoder)
                    } label: {
                        Text(credits)
                            .font(ConsoleType.micro)
                            .foregroundStyle(ConsoleTheme.accent)
                            .lineLimit(1)
                            .minimumScaleFactor(0.8)
                    }
                    .buttonStyle(.plain)
                    .disabled(!row.creditsNavigatesToCheckin)
                    .help(row.creditsHelp ?? "")
                }
            }
            .frame(minWidth: Column.accountMin, maxWidth: .infinity, alignment: .leading)

            VStack(alignment: .leading, spacing: 3) {
                HStack(alignment: .firstTextBaseline, spacing: 4) {
                    Text(row.remainingValue)
                        .font(ConsoleType.rowTitle)
                        .monospacedDigit()
                        .foregroundStyle(row.remainingValue == "—" || row.remainingValue == "未知"
                                         ? ConsoleTheme.secondaryText : ConsoleTheme.text)
                        .lineLimit(1)
                        .minimumScaleFactor(0.65)
                    if let unit = row.unit {
                        Text(unit)
                            .font(ConsoleType.micro)
                            .foregroundStyle(ConsoleTheme.tertiaryText)
                            .lineLimit(1)
                            .minimumScaleFactor(0.65)
                    }
                }
                if let share = row.remainingShare {
                    ConsoleShareBar(share: share, tone: shareTone(share))
                        .frame(maxWidth: 96)
                }
            }
            .frame(width: Column.accountRemaining, alignment: .leading)
            .help([row.remainingHelp, row.sourceHelp].compactMap { $0 }.joined(separator: "\n"))

            VStack(alignment: .leading, spacing: 2) {
                Text(row.usagePrimary)
                    .font(ConsoleType.body)
                    .foregroundStyle(row.usagePrimary == "读取失败" ? ConsoleTheme.warning : ConsoleTheme.text)
                    .lineLimit(1)
                if let secondary = row.usageSecondary {
                    Text(secondary)
                        .font(ConsoleType.micro)
                        .foregroundStyle(ConsoleTheme.tertiaryText)
                        .lineLimit(1)
                        .minimumScaleFactor(0.8)
                        .help(secondary)
                }
            }
            .frame(width: Column.accountUsage, alignment: .leading)
            .help(row.usageHelp ?? "")

            Text(row.updatedText ?? "—")
                .font(ConsoleType.micro)
                .foregroundStyle(ConsoleTheme.tertiaryText)
                .monospacedDigit()
                .lineLimit(1)
                .minimumScaleFactor(0.8)
                .frame(width: Column.accountUpdated, alignment: .leading)

            statusCell(row.state)
                .frame(width: Column.status, alignment: .leading)
        }
        .padding(.horizontal, Column.inset)
        .padding(.vertical, 8)
        .frame(minHeight: ConsoleTheme.rowHeight)
    }

    // MARK: - 共用单元格

    private func headerCell(_ title: String, width: CGFloat?) -> some View {
        Text(title)
            .font(ConsoleType.micro)
            .foregroundStyle(ConsoleTheme.tertiaryText)
            .lineLimit(1)
            .frame(width: width, alignment: .leading)
            .frame(maxWidth: width == nil ? .infinity : nil, alignment: .leading)
    }

    private func statusCell(_ state: QuotaDisplayState) -> some View {
        Group {
            if let label = state.label {
                Text(label)
                    .font(ConsoleType.micro)
                    .foregroundStyle(stateColor(state))
                    .lineLimit(1)
                    .minimumScaleFactor(0.8)
                    .help(state.help ?? "")
            } else {
                // 正常状态不写字：只留一个弱提示点，异常才占用这一列。
                Circle()
                    .fill(ConsoleTheme.success)
                    .frame(width: 5, height: 5)
                    .help(state.help ?? "实时数据")
            }
        }
    }

    private func stateColor(_ state: QuotaDisplayState) -> Color {
        switch state.tone {
        case .normal: return ConsoleTheme.secondaryText
        case .neutral: return ConsoleTheme.tertiaryText
        case .warning: return ConsoleTheme.warning
        case .danger: return ConsoleTheme.danger
        }
    }

    private func shareTone(_ share: Double) -> Color {
        if share <= 10 { return ConsoleTheme.danger }
        if share <= 30 { return ConsoleTheme.warning }
        return ConsoleTheme.accent
    }

    // MARK: - 空 / 错误

    private func emptyState(title: String, message: String, action: @escaping () -> Void) -> some View {
        ConsoleEmptyState(
            symbol: "chart.bar.doc.horizontal",
            title: title,
            message: message,
            actionTitle: "刷新",
            action: action
        )
    }

    private func errorState(title: String, message: String, action: @escaping () -> Void) -> some View {
        ConsoleEmptyState(
            symbol: "exclamationmark.triangle",
            title: title,
            message: message,
            actionTitle: "重新读取",
            action: action
        )
    }

    private func filteredEmptyState(title: String, message: String, actionTitle: String, action: @escaping () -> Void) -> some View {
        ConsoleEmptyState(
            symbol: "line.3.horizontal.decrease.circle",
            title: title,
            message: message,
            actionTitle: actionTitle,
            action: action
        )
    }

    /// 供其他控制台区块复用的时间格式化（保留既有调用点）。
    static func localTimestamp(_ raw: String) -> String {
        QuotaPresentation.localTimestamp(raw)
            ?? String(raw.prefix(16)).replacingOccurrences(of: "T", with: " ")
    }
}

