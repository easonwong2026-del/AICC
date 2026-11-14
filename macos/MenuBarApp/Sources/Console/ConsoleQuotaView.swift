import SwiftUI

/// 额度看板：完整搬用 Widget 的数据链路（`WidgetStatusLoader` + `QuotaSnapshot.slots`），
/// 不重新实现 Magpie 读取，也不改 Widget schema。
struct ConsoleQuotaView: View {
    @ObservedObject var store: ConsoleStore

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            summaryRow
            workerSection
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
    }

    // MARK: - 助手侧四平台额度（M4）

    /// 上面那段是 Magpie 的额度窗口，这段是助手直接问官方接口的账号级余额与用量。
    /// 两段数据来源不同，不合并、不相加。
    @ViewBuilder
    private var workerSection: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                ConsoleSectionTitle(
                    title: "账号额度（官方接口）",
                    detail: store.workerQuotaRows.isEmpty ? nil : "\(store.workerQuotaRows.count) 个账号"
                )
                Spacer(minLength: 8)
                if store.loadingWorkerQuota {
                    ProgressView().controlSize(.small)
                }
                Button("刷新") {
                    Task { await store.refreshWorkerQuota(force: true) }
                }
                .buttonStyle(ConsoleSecondaryButtonStyle())
                .disabled(store.loadingWorkerQuota)
            }

            if store.workerQuotaRows.isEmpty {
                ConsoleCard {
                    ConsoleEmptyState(
                        symbol: "gauge.with.dots.needle.33percent",
                        title: "没有读到账号额度",
                        message: store.workerQuotaError ?? "助手还没返回任何账号。",
                        actionTitle: "重新读取",
                        action: { Task { await store.refreshWorkerQuota(force: true) } }
                    )
                }
            } else {
                LazyVGrid(
                    columns: [GridItem(.adaptive(minimum: 320), spacing: ConsoleTheme.cardSpacing)],
                    spacing: ConsoleTheme.cardSpacing
                ) {
                    ForEach(store.workerQuotaRows) { row in
                        workerQuotaCard(row)
                    }
                }
            }
        }
    }

    private func workerQuotaCard(_ row: WorkerQuotaRow) -> some View {
        ConsoleCard {
            VStack(alignment: .leading, spacing: 10) {
                HStack(alignment: .firstTextBaseline, spacing: 8) {
                    Text(row.nickname.isEmpty ? row.accountId : row.nickname)
                        .font(.system(size: 12, weight: .semibold))
                        .foregroundStyle(ConsoleTheme.text)
                        .lineLimit(1)
                    ConsoleBadge(text: row.providerName, tint: ConsoleTheme.secondaryText)
                    Spacer(minLength: 0)
                    if row.stale {
                        // 旧数据 + 这次为什么没取到，两件事都要可见。
                        ConsoleBadge(text: "过期", tint: ConsoleTheme.danger)
                    } else if row.cached {
                        ConsoleBadge(text: "缓存", tint: ConsoleTheme.warning)
                    }
                }

                switch row.status {
                case "ok", "stale":
                    if let text = row.remainingText {
                        HStack(alignment: .firstTextBaseline, spacing: 6) {
                            Text(text)
                                .font(.system(size: 18, weight: .semibold))
                                .monospacedDigit()
                                .foregroundStyle(ConsoleTheme.text)
                            if let unit = row.unit {
                                Text(unit)
                                    .font(.system(size: 10))
                                    .foregroundStyle(ConsoleTheme.tertiaryText)
                            }
                            Spacer(minLength: 0)
                            if let plan = row.plan, !plan.isEmpty {
                                ConsoleBadge(text: plan, tint: ConsoleTheme.secondaryText)
                            }
                        }
                        if let share = row.remainingShare {
                            ConsoleShareBar(share: share, tone: share <= 30 ? ConsoleTheme.warning : ConsoleTheme.accent)
                        }
                        if let total = row.totalText {
                            Text(total)
                                .font(.system(size: 10))
                                .foregroundStyle(ConsoleTheme.tertiaryText)
                        }
                    } else {
                        // 拿到 200 但没拿到额度：显示未知，不显示 0。
                        Text("额度未知")
                            .font(.system(size: 12))
                            .foregroundStyle(ConsoleTheme.secondaryText)
                    }
                    if let usage = row.usage, usage.requests > 0 {
                        Text("近 \(usage.windowDays) 天 \(usage.requests) 次请求 · 消耗 \(WorkerQuotaRow.format(usage.credits))\(usage.truncated ? "（已截断）" : "")")
                            .font(.system(size: 10))
                            .foregroundStyle(ConsoleTheme.tertiaryText)
                    } else if let usageError = row.usageError, !usageError.isEmpty {
                        // 余额拿到了、用量没拿到：单独一行说明，不影响上面的额度。
                        Text("用量读取失败：\(usageError)")
                            .font(.system(size: 10))
                            .foregroundStyle(ConsoleTheme.warning)
                    }
                case "unverified":
                    Text("未接入（已验证为零请求）")
                        .font(.system(size: 12))
                        .foregroundStyle(ConsoleTheme.secondaryText)
                case "unauthorized":
                    Text("授权不可用")
                        .font(.system(size: 12))
                        .foregroundStyle(ConsoleTheme.danger)
                default:
                    Text("读取失败")
                        .font(.system(size: 12))
                        .foregroundStyle(ConsoleTheme.danger)
                }

                HStack(spacing: 6) {
                    Text(row.sourceName)
                    if let capturedAt = row.capturedAt, !capturedAt.isEmpty {
                        Text("· 数据于 \(capturedAt.prefix(16).replacingOccurrences(of: "T", with: " "))")
                    }
                    Spacer(minLength: 0)
                }
                .font(.system(size: 10))
                .foregroundStyle(ConsoleTheme.tertiaryText)

                if let error = row.error, !error.isEmpty {
                    Text(error)
                        .font(.system(size: 10))
                        .foregroundStyle(ConsoleTheme.tertiaryText)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
        }
    }

    private var summaryRow: some View {
        HStack(spacing: 10) {
            if let snapshot = store.snapshot {
                ConsoleBadge(
                    text: snapshot.stale ? "缓存数据" : "实时数据",
                    tint: snapshot.stale ? ConsoleTheme.warning : ConsoleTheme.success
                )
                Text("\(snapshot.providers.count) 个服务商 · \(snapshot.accounts.count) 张额度卡")
                    .font(.system(size: 11))
                    .foregroundStyle(ConsoleTheme.secondaryText)
                if let updated = snapshot.updatedAt {
                    Text("更新于 \(updated)")
                        .font(.system(size: 10))
                        .foregroundStyle(ConsoleTheme.tertiaryText)
                }
            } else {
                Text("尚未读取 Magpie")
                    .font(.system(size: 11))
                    .foregroundStyle(ConsoleTheme.secondaryText)
            }

            Spacer(minLength: 8)

            if store.loadingQuota {
                ProgressView().controlSize(.small)
            }
            Button("刷新额度") {
                Task { await store.refreshQuota(force: true) }
            }
            .buttonStyle(ConsoleSecondaryButtonStyle())
            .disabled(store.loadingQuota)
        }
    }

    private func providerSections(_ snapshot: QuotaSnapshot) -> some View {
        let groups = groupedByProvider(snapshot)
        return VStack(alignment: .leading, spacing: 20) {
            ForEach(groups, id: \.provider) { group in
                VStack(alignment: .leading, spacing: 10) {
                    ConsoleSectionTitle(
                        title: group.name,
                        detail: "\(group.cards.count) 个账号"
                    )
                    LazyVGrid(
                        columns: [GridItem(.adaptive(minimum: 320), spacing: ConsoleTheme.cardSpacing)],
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

    private struct ProviderGroup {
        let provider: String
        let name: String
        let cards: [QuotaCard]
    }

    /// 按服务商分组，保持快照里的原始顺序（Magpie 已按紧迫程度排过）。
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
        return order.map { provider in
            let cards = buckets[provider] ?? []
            return ProviderGroup(
                provider: provider,
                name: cards.first?.providerName ?? MagpieQuotaParser.providerName(provider),
                cards: cards
            )
        }
    }

    private func quotaCard(_ card: QuotaCard) -> some View {
        ConsoleCard {
            VStack(alignment: .leading, spacing: 12) {
                HStack(alignment: .firstTextBaseline, spacing: 8) {
                    Text(card.accountLabel)
                        .font(.system(size: 12, weight: .semibold))
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
                        .font(.system(size: 11))
                        .foregroundStyle(ConsoleTheme.secondaryText)
                } else {
                    VStack(spacing: 10) {
                        ForEach(Array(card.items.enumerated()), id: \.offset) { _, item in
                            itemRow(item)
                        }
                    }
                }

                if let status = card.statusText, !status.isEmpty {
                    Text(status)
                        .font(.system(size: 10))
                        .foregroundStyle(ConsoleTheme.secondaryText)
                }
            }
        }
    }

    private func itemRow(_ item: QuotaItem) -> some View {
        VStack(alignment: .leading, spacing: 5) {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Text(item.name)
                    .font(.system(size: 11))
                    .foregroundStyle(ConsoleTheme.secondaryText)
                    .lineLimit(1)
                Spacer(minLength: 6)
                Text(item.primaryText)
                    .font(.system(size: 12, weight: .semibold))
                    .monospacedDigit()
                    .foregroundStyle(ConsoleTheme.text)
            }
            if let share = item.remaining, item.isPercentage {
                ConsoleShareBar(share: share, tone: barTone(share))
            }
            HStack(spacing: 8) {
                if let total = item.totalText {
                    Text(total)
                        .font(.system(size: 10))
                        .foregroundStyle(ConsoleTheme.tertiaryText)
                }
                if let reset = item.reset, !reset.isEmpty {
                    Text("重置于 \(reset)")
                        .font(.system(size: 10))
                        .foregroundStyle(ConsoleTheme.tertiaryText)
                }
                Spacer(minLength: 0)
            }
        }
    }

    private func balanceRows(_ card: QuotaCard) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(card.balanceText)
                .font(.system(size: 18, weight: .semibold))
                .monospacedDigit()
                .foregroundStyle(ConsoleTheme.text)
            ForEach(Array(card.balanceParts.enumerated()), id: \.offset) { _, part in
                HStack(spacing: 6) {
                    Text(part.label ?? "余额")
                        .font(.system(size: 10))
                        .foregroundStyle(ConsoleTheme.secondaryText)
                    Text(part.text)
                        .font(.system(size: 10))
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
}
