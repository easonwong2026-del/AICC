import SwiftUI

struct ConsoleOverviewView: View {
    @ObservedObject var store: ConsoleStore
    let navigate: (ConsolePage) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            statsRow
            accountsSection
            quotaSection
            systemSection
        }
    }

    // MARK: 顶部统计

    private var statsRow: some View {
        HStack(spacing: ConsoleTheme.cardSpacing) {
            ConsoleStatTile(
                caption: "账号总数",
                value: "\(store.stats.total)",
                hint: "WorkBuddy \(store.stats.workbuddy) · Qoder CN \(store.stats.qoder)"
            )
            ConsoleStatTile(
                caption: "认证正常",
                value: "\(store.stats.healthy)",
                hint: store.stats.attention > 0 ? "\(store.stats.attention) 个需要重新登录" : "没有异常账号",
                tone: store.stats.attention > 0 ? ConsoleTheme.warning : ConsoleTheme.text
            )
            ConsoleStatTile(
                caption: "今日签到",
                value: "待接入",
                hint: "PR1b 联调后开放",
                tone: ConsoleTheme.secondaryText
            )
        }
    }

    // MARK: AI 账号

    private var accountsSection: some View {
        VStack(alignment: .leading, spacing: 10) {
            ConsoleSectionTitle(
                title: "AI 账号",
                detail: "共 \(store.stats.total + store.qoderProfiles.count) 个"
            )
            HStack(spacing: ConsoleTheme.cardSpacing) {
                platformCard(
                    name: "WorkBuddy",
                    platform: "workbuddy",
                    symbol: "bubble.left.and.bubble.right.fill"
                )
                // 两个 Qoder 版本的数据源是 Profile，不再是 accounts 表。
                profileCard(edition: .cn, symbol: "chevron.left.forwardslash.chevron.right")
                profileCard(edition: .global, symbol: "globe")
            }
        }
    }

    /// Qoder 版本卡：数量来自 Profile，登录态来自每个 Profile 的 authStatus。
    private func profileCard(edition: QoderEdition, symbol: String) -> some View {
        let rows = store.qoderProfiles.filter { $0.edition == edition.rawValue }
        let healthy = rows.filter { $0.authStatus == "valid" }.count
        let pending = rows.count - healthy
        let running = rows.filter { $0.runtime.isRunning }.count

        return ConsoleCard {
            VStack(alignment: .leading, spacing: 12) {
                HStack(spacing: 9) {
                    Image(systemName: symbol)
                        .font(.system(size: 12, weight: .semibold))
                        .foregroundStyle(ConsoleTheme.accent)
                        .frame(width: 26, height: 26)
                        .background(ConsoleTheme.accentSoft, in: RoundedRectangle(cornerRadius: 7))
                    Text(edition.displayName)
                        .font(.system(size: 13, weight: .semibold))
                        .foregroundStyle(ConsoleTheme.text)
                        .lineLimit(1)
                    Spacer(minLength: 0)
                    Text("\(rows.count)")
                        .font(.system(size: 17, weight: .semibold))
                        .monospacedDigit()
                        .foregroundStyle(ConsoleTheme.text)
                }

                HStack(spacing: 10) {
                    ConsoleStatusDot(color: ConsoleTheme.success, text: "已登录 \(healthy)")
                    if pending > 0 {
                        ConsoleStatusDot(color: ConsoleTheme.warning, text: "待处理 \(pending)")
                    } else {
                        ConsoleStatusDot(color: ConsoleTheme.tertiaryText, text: "无异常")
                    }
                    if running > 0 {
                        ConsoleStatusDot(color: ConsoleTheme.accent, text: "运行中 \(running)")
                    }
                }

                Button("查看账号") { navigate(.accounts) }
                    .buttonStyle(ConsoleSecondaryButtonStyle())
            }
        }
    }

    private func platformCard(name: String, platform: String, symbol: String) -> some View {
        let rows = store.accounts.filter { $0.platform == platform }
        let healthy = rows.filter { $0.authStatus == "valid" }.count
        let pending = rows.count - healthy

        return ConsoleCard {
            VStack(alignment: .leading, spacing: 12) {
                HStack(spacing: 9) {
                    Image(systemName: symbol)
                        .font(.system(size: 12, weight: .semibold))
                        .foregroundStyle(ConsoleTheme.accent)
                        .frame(width: 26, height: 26)
                        .background(ConsoleTheme.accentSoft, in: RoundedRectangle(cornerRadius: 7))
                    Text(name)
                        .font(.system(size: 13, weight: .semibold))
                        .foregroundStyle(ConsoleTheme.text)
                    Spacer(minLength: 0)
                    Text("\(rows.count)")
                        .font(.system(size: 17, weight: .semibold))
                        .monospacedDigit()
                        .foregroundStyle(ConsoleTheme.text)
                }

                HStack(spacing: 10) {
                    ConsoleStatusDot(color: ConsoleTheme.success, text: "正常 \(healthy)")
                    if pending > 0 {
                        ConsoleStatusDot(color: ConsoleTheme.warning, text: "待处理 \(pending)")
                    } else {
                        ConsoleStatusDot(color: ConsoleTheme.tertiaryText, text: "无异常")
                    }
                }

                Button("查看账号") { navigate(.accounts) }
                    .buttonStyle(ConsoleSecondaryButtonStyle())
            }
        }
    }

    // MARK: 额度概览

    private var quotaSection: some View {
        VStack(alignment: .leading, spacing: 10) {
            ConsoleSectionTitle(
                title: "额度概览",
                detail: store.quotaSummary.isEmpty
                    ? "Magpie 暂无数据"
                    : "\(store.quotaSummary.providerCount) 个服务商 · \(store.quotaSummary.cardCount) 张卡"
            )
            ConsoleCard(padding: 0) {
                if let snapshot = store.snapshot, !snapshot.accounts.isEmpty {
                    VStack(spacing: 0) {
                        ForEach(topSlots(from: snapshot), id: \.card.id) { slot in
                            slotRow(slot)
                            if slot.card.id != topSlots(from: snapshot).last?.card.id {
                                Divider().overlay(ConsoleTheme.border).padding(.leading, 16)
                            }
                        }
                        Divider().overlay(ConsoleTheme.border)
                        HStack {
                            if snapshot.stale {
                                ConsoleBadge(text: "缓存", tint: ConsoleTheme.warning)
                            }
                            Text(snapshot.updatedAt.map { "更新于 \($0)" } ?? "刚更新")
                                .font(.system(size: 10))
                                .foregroundStyle(ConsoleTheme.tertiaryText)
                            Spacer()
                            Button("查看全部") { navigate(.quota) }
                                .buttonStyle(.borderless)
                                .font(.system(size: 11, weight: .medium))
                                .foregroundStyle(ConsoleTheme.accent)
                        }
                        .padding(.horizontal, 16)
                        .padding(.vertical, 9)
                    }
                } else {
                    ConsoleEmptyState(
                        symbol: "chart.bar.doc.horizontal",
                        title: "还没有读到额度",
                        message: store.quotaError ?? "启动 Magpie 后这里会显示各服务商的额度窗口。",
                        actionTitle: "重新读取",
                        action: { Task { await store.refreshQuota(force: true) } }
                    )
                }
            }
        }
    }

    private func topSlots(from snapshot: QuotaSnapshot) -> [QuotaSlot] {
        Array(snapshot.slots(selection: AppSettings.shared.widgetSelection, limit: 3).shown.prefix(3))
    }

    private func slotRow(_ slot: QuotaSlot) -> some View {
        HStack(spacing: 12) {
            VStack(alignment: .leading, spacing: 3) {
                Text(slot.card.title)
                    .font(.system(size: 12, weight: .medium))
                    .foregroundStyle(ConsoleTheme.text)
                    .lineLimit(1)
                Text(slot.primary.name)
                    .font(.system(size: 10))
                    .foregroundStyle(ConsoleTheme.secondaryText)
                    .lineLimit(1)
            }
            Spacer(minLength: 8)
            VStack(alignment: .trailing, spacing: 5) {
                Text(slot.primary.primaryText)
                    .font(.system(size: 12, weight: .semibold))
                    .monospacedDigit()
                    .foregroundStyle(ConsoleTheme.text)
                if let share = slot.primary.remaining, slot.primary.isPercentage {
                    ConsoleShareBar(share: share, tone: barTone(share))
                        .frame(width: 92)
                }
            }
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 9)
    }

    private func barTone(_ share: Double) -> Color {
        if share <= 10 { return ConsoleTheme.danger }
        if share <= 30 { return ConsoleTheme.warning }
        return ConsoleTheme.accent
    }

    // MARK: 系统状态

    private var systemSection: some View {
        VStack(alignment: .leading, spacing: 10) {
            ConsoleSectionTitle(title: "系统状态")
            ConsoleCard {
                HStack(spacing: 18) {
                    VStack(alignment: .leading, spacing: 8) {
                        ConsoleStatusDot(color: store.magpieHealthColor, text: store.magpieHealthText)
                        ConsoleStatusDot(color: store.workerHealthColor, text: store.workerHealthText)
                    }
                    Spacer(minLength: 12)
                    HStack(spacing: 8) {
                        Button("刷新额度") {
                            Task { await store.refreshQuota(force: true) }
                        }
                        .buttonStyle(ConsoleSecondaryButtonStyle())

                        Button("执行签到") {}
                            .buttonStyle(ConsoleSecondaryButtonStyle())
                            .disabled(true)
                            .help("PR1b 联调后开放")
                    }
                }
            }
        }
    }
}
