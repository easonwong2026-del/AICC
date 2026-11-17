import SwiftUI

/// 账号详情里的**「奖励」区块**（PR #98）。
///
/// 数据来自 `growth.rewards.status`（只读，四个 GET）。这里展示：
/// 三档连登、抽奖次数、补签卡与可补签日期、本地账本里的历史操作。
///
/// **抽奖 / 兑换 / 补签三个按钮现在是禁用状态**，理由写在按钮下面：三个写接口
/// 都还没拿到 AICC 自己的 A 级证据（见 `docs/growth-rewards-protocol-2026-11-16.md` §八）。
/// 放一个点了没反应的按钮，比不放按钮更糟 —— 用户会以为"点了没生效"，
/// 而真实原因是"我们还没接线"。
struct ConsoleRewardsBlock: View {
    let state: RewardAccountState?
    let loading: Bool
    let failure: String?
    let onRefresh: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            headerRow
            if let state {
                tierRow(state)
                metricsRow(state)
                makeupRow(state)
                if let notice = state.awaitingText { noticeRow(notice, tone: ConsoleTheme.warning) }
                if let notice = state.ledgerUnreadableText { noticeRow(notice, tone: ConsoleTheme.danger) }
                if let note = state.makeup.noteText, state.makeup.hasWorkToDo {
                    Text(note)
                        .font(ConsoleType.caption)
                        .foregroundStyle(ConsoleTheme.tertiaryText)
                }
                errorRows(state)
                Divider().overlay(ConsoleTheme.border)
                operations(state)
                Divider().overlay(ConsoleTheme.border)
                actions(state)
            } else if let failure {
                Text(failure)
                    .font(ConsoleType.small)
                    .foregroundStyle(ConsoleTheme.danger)
                    .fixedSize(horizontal: false, vertical: true)
            } else {
                Text(loading ? "正在读取奖励状态…" : "还没读取。点「刷新」查一次（只读，不会消耗任何次数）。")
                    .font(ConsoleType.small)
                    .foregroundStyle(ConsoleTheme.secondaryText)
            }
        }
    }

    // MARK: - 头部

    private var headerRow: some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            Text(state?.summaryText ?? "—")
                .font(ConsoleType.sectionDetail)
                .foregroundStyle(ConsoleTheme.secondaryText)
            Spacer(minLength: 8)
            if let state {
                ConsoleBadge(text: state.stateLabel, tint: tone(for: state.state))
            }
            Button(loading ? "读取中…" : "刷新") { onRefresh() }
                .buttonStyle(ConsoleSecondaryButtonStyle())
                .disabled(loading)
                .help("只读查询：连登档位、抽奖次数、补签卡。不会抽奖、兑换或补签")
        }
    }

    // MARK: - 三档

    private func tierRow(_ state: RewardAccountState) -> some View {
        HStack(spacing: 8) {
            ForEach(state.tiers) { tier in
                VStack(alignment: .leading, spacing: 2) {
                    Text(tier.title)
                        .font(ConsoleType.rowTitle)
                        .foregroundStyle(ConsoleTheme.text)
                    Text(tier.stateLabel)
                        .font(ConsoleType.caption)
                        .foregroundStyle(tone(for: tier.state))
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.horizontal, 10)
                .padding(.vertical, 8)
                .background(ConsoleTheme.cardSubtle, in: RoundedRectangle(cornerRadius: 8))
                .overlay(
                    RoundedRectangle(cornerRadius: 8)
                        .stroke(tier.claimable ? tone(for: tier.state).opacity(0.5) : ConsoleTheme.border)
                )
            }
        }
    }

    // MARK: - 指标

    private func metricsRow(_ state: RewardAccountState) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            ConsoleInfoRow(label: "连登天数", value: state.streakDays.map { "\($0) 天" } ?? "—")
            ConsoleInfoRow(label: "抽奖次数", value: state.lotteryChances.map { "\($0) 次" } ?? "—")
            ConsoleInfoRow(label: "补签卡", value: state.makeup.balanceText)
            if let today = state.makeup.today {
                ConsoleInfoRow(label: "服务端日期", value: today, mono: true)
            }
        }
    }

    private func makeupRow(_ state: RewardAccountState) -> some View {
        let candidates = state.makeup.candidates
        return VStack(alignment: .leading, spacing: 4) {
            if candidates.isEmpty {
                Text("可补签日期：无")
                    .font(ConsoleType.small)
                    .foregroundStyle(ConsoleTheme.secondaryText)
            } else {
                Text("可补签 \(candidates.count) 天：\(preview(candidates))")
                    .font(ConsoleType.small)
                    .foregroundStyle(ConsoleTheme.text)
                    .fixedSize(horizontal: false, vertical: true)
                Text("这些日期由服务端日历（score = 0）与活动规则算出；点「补签」之前还会再查一次。")
                    .font(ConsoleType.caption)
                    .foregroundStyle(ConsoleTheme.tertiaryText)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }

    private func preview(_ dates: [String]) -> String {
        let head = dates.prefix(5).map { $0.suffix(5) } // MM-DD
        let rest = dates.count - head.count
        return head.joined(separator: "、") + (rest > 0 ? " 等 \(dates.count) 天" : "")
    }

    private func noticeRow(_ text: String, tone: Color) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 6) {
            Image(systemName: "exclamationmark.triangle.fill")
                .font(.system(size: 10))
                .foregroundStyle(tone)
            Text(text)
                .font(ConsoleType.caption)
                .foregroundStyle(tone)
                .fixedSize(horizontal: false, vertical: true)
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 6)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(tone.opacity(0.1), in: RoundedRectangle(cornerRadius: 8))
    }

    private func errorRows(_ state: RewardAccountState) -> some View {
        VStack(alignment: .leading, spacing: 3) {
            ForEach(Array(state.errors.prefix(3))) { error in
                Text("\(label(for: error.field))：\(error.message)")
                    .font(ConsoleType.caption)
                    .foregroundStyle(error.needsUserAction ? ConsoleTheme.danger : ConsoleTheme.tertiaryText)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }

    private func label(for field: String) -> String {
        switch field {
        case "streak": return "连登状态"
        case "redeem": return "档位状态"
        case "lottery": return "抽奖次数"
        case "heatmap": return "活跃日历"
        case "ledger": return "本地账本"
        case "credential": return "凭据"
        default: return field
        }
    }

    // MARK: - 历史

    private func operations(_ state: RewardAccountState) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 6) {
                Text("执行历史")
                    .font(ConsoleType.sectionTitle)
                    .foregroundStyle(ConsoleTheme.text)
                Text("本地账本，最多 5 条")
                    .font(ConsoleType.caption)
                    .foregroundStyle(ConsoleTheme.tertiaryText)
                Spacer(minLength: 0)
            }
            if state.recentOperations.isEmpty {
                Text("还没有记录。AICC 每次发出抽奖、兑换、补签之前都会先记账本。")
                    .font(ConsoleType.caption)
                    .foregroundStyle(ConsoleTheme.tertiaryText)
                    .fixedSize(horizontal: false, vertical: true)
            } else {
                ForEach(state.recentOperations) { operation in
                    HStack(alignment: .firstTextBaseline, spacing: 8) {
                        Text(operation.actionLabel)
                            .font(ConsoleType.small)
                            .foregroundStyle(ConsoleTheme.text)
                            .frame(width: 64, alignment: .leading)
                        if !operation.targetLabel.isEmpty {
                            Text(operation.targetLabel)
                                .font(ConsoleType.mono)
                                .foregroundStyle(ConsoleTheme.secondaryText)
                        }
                        ConsoleBadge(
                            text: operation.phaseLabel,
                            tint: tone(for: operation.phase)
                        )
                        Spacer(minLength: 0)
                        Text(ConsoleFormat.shortTimestamp(operation.createdAt))
                            .font(ConsoleType.caption)
                            .foregroundStyle(ConsoleTheme.tertiaryText)
                    }
                }
            }
        }
    }

    // MARK: - 操作

    private func actions(_ state: RewardAccountState) -> some View {
        let claimable = state.claimableTiers
        let redeemTitle = claimable.isEmpty
            ? "兑换连登奖励"
            : "兑换连登奖励（\(claimable.map(\.tier).joined(separator: " / "))）"
        return VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 8) {
                Button("抽奖") {}
                    .buttonStyle(ConsoleSecondaryButtonStyle())
                    .disabled(true)
                    .help("抽奖接口还没拿到 A 级证据，未接线")
                Button(redeemTitle) {}
                    .buttonStyle(ConsoleSecondaryButtonStyle())
                    .disabled(true)
                    .help("兑换接口还没拿到 A 级证据，未接线")
                Button("补签") {}
                    .buttonStyle(ConsoleSecondaryButtonStyle())
                    .disabled(true)
                    .help("补签接口还没拿到 A 级证据，未接线")
                Spacer(minLength: 0)
            }
            Text("这三个动作会消耗次数或补签卡，接口证据到位前保持禁用；接好后每一次都会先记本地账本、再回查服务端。")
                .font(ConsoleType.caption)
                .foregroundStyle(ConsoleTheme.tertiaryText)
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    // MARK: - 配色

    private func tone(for state: String) -> Color {
        switch state {
        case "claimable", "confirmed": return ConsoleTheme.success
        case "locked", "rejected", "ended": return ConsoleTheme.secondaryText
        case "claimed": return ConsoleTheme.success
        case "unknown", "sent", "pending", "ready": return ConsoleTheme.warning
        case "frozen", "failed", "unauthorized", "missing_credential": return ConsoleTheme.danger
        case "partial": return ConsoleTheme.warning
        default: return ConsoleTheme.secondaryText
        }
    }

    private func tone(for tier: RewardTierState) -> Color {
        switch tier.state {
        case "claimable": return ConsoleTheme.success
        case "claimed": return ConsoleTheme.secondaryText
        case "locked": return ConsoleTheme.tertiaryText
        case "ended": return ConsoleTheme.tertiaryText
        default: return ConsoleTheme.warning
        }
    }
}
