import SwiftUI

/// 账号详情里的**「奖励」区块**（PR #98）。
///
/// 数据来自 `growth.rewards.status`（只读，四个 GET）。这里展示：
/// 三档连登、抽奖次数、补签卡与可补签日期、本地账本里的历史操作。
///
/// **三个写动作（抽奖 / 兑换 / 补签）已接线**（PR #101），流程固定：
/// 点按钮 → 先拉一次实时余额与资格（`confirm: false`，不发写请求）→ 二次确认
/// → 记本地账本 → 只发一次写请求 → 回查服务端。结果未知时只能「回查」，
/// 不会自动重发；账本读不到时显示"无法确认"而不是 0 条。
struct ConsoleRewardsBlock: View {
    let state: RewardAccountState?
    let loading: Bool
    let failure: String?
    /// 上一次写操作的结果（成功/拒绝/未定/冻结）。
    var result: RewardActionResult? = nil
    var acting: Bool = false
    /// 点按钮后现拉的预览；只有它到达才弹确认框。
    var preview: RewardPreview? = nil
    var checking: Bool = false
    let onRefresh: () -> Void
    var onPreview: (RewardAction, String?) -> Void = { _, _ in }
    var onConfirmPreview: () -> Void = {}
    var onCancelPreview: () -> Void = {}
    var onRecheck: () -> Void = {}
    var onDismissResult: () -> Void = {}

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            headerRow
            if let state {
                if let notice = RewardRefreshNotice.make(state: state, failure: failure) {
                    staleBanner(notice)
                }
                if let result { resultBanner(result) }
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
                Text("上次读取 \(ConsoleFormat.shortTimestamp(state.scannedAt))")
                    .font(ConsoleType.caption)
                    .foregroundStyle(ConsoleTheme.tertiaryText)
                if RewardRefreshNotice.make(state: state, failure: failure) != nil {
                    // 旧数据 + 本次失败：状态徽章换成「显示旧数据」，别让"已读取"与红字冲突。
                    ConsoleBadge(text: "显示旧数据", tint: ConsoleTheme.warning)
                } else {
                    ConsoleBadge(text: state.stateLabel, tint: tone(for: state.state))
                }
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
            if let note = state.streakNote {
                Text(note)
                    .font(ConsoleType.caption)
                    .foregroundStyle(ConsoleTheme.tertiaryText)
                    .fixedSize(horizontal: false, vertical: true)
            }
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

    /// 刷新失败但还留着旧数据时的横幅。**必须比普通提示更显眼**：
    /// 用户看到的数字是旧的，不说明白就会把它当实时状态。
    private func staleBanner(_ notice: RewardRefreshNotice) -> some View {
        VStack(alignment: .leading, spacing: 3) {
            Text(RewardRefreshNotice.title)
                .font(ConsoleType.rowTitle)
                .foregroundStyle(ConsoleTheme.warning)
            Text(notice.reason)
                .font(ConsoleType.caption)
                .foregroundStyle(ConsoleTheme.secondaryText)
                .fixedSize(horizontal: false, vertical: true)
            Text(notice.hasReadAt
                 ? "上次成功读取：\(ConsoleFormat.shortTimestamp(notice.readAt))"
                 : "上次成功读取时间未知")
                .font(ConsoleType.caption)
                .foregroundStyle(ConsoleTheme.tertiaryText)
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 8)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(ConsoleTheme.warning.opacity(0.12), in: RoundedRectangle(cornerRadius: 8))
        .overlay(
            RoundedRectangle(cornerRadius: 8)
                .stroke(ConsoleTheme.warning.opacity(0.35))
        )
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
        let claimable = state.claimableTiers.first
        let redeemTitle = claimable.map { "兑换连登奖励（\($0.tier)）" } ?? "兑换连登奖励"
        let needsRecheck = state.recentOperations.contains { $0.needsAttention }
        return VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 8) {
                Button("抽奖") { onPreview(.draw, nil) }
                    .buttonStyle(ConsoleSecondaryButtonStyle())
                    // 余额未知（nil）也要禁用：不能拿"不知道"当"有次数"。
                    .disabled(acting || !RewardAction.draw.isEnabled(for: state))
                Button(redeemTitle) {
                    if let tier = claimable?.tier { onPreview(.redeem, tier) }
                }
                .buttonStyle(ConsoleSecondaryButtonStyle())
                .disabled(acting || !RewardAction.redeem.isEnabled(for: state))
                Button("补签") {
                    if let date = state.makeup.candidates.first { onPreview(.makeup, date) }
                }
                .buttonStyle(ConsoleSecondaryButtonStyle())
                .disabled(acting || !RewardAction.makeup.isEnabled(for: state))
                if needsRecheck {
                    Button(checking ? "回查中…" : "回查结果") { onRecheck() }
                        .buttonStyle(ConsoleSecondaryButtonStyle())
                        .disabled(checking || acting)
                        .help("只读：向服务端确认上一次请求的结果，不会重发")
                }
                Spacer(minLength: 0)
            }
            Text("点击后先显示当前余额，再让你确认；确认后只执行一次。"
                + "结果没有立刻确认时可以回查，不会重复执行。")
                .font(ConsoleType.caption)
                .foregroundStyle(ConsoleTheme.tertiaryText)
                .fixedSize(horizontal: false, vertical: true)
        }
        .confirmationDialog(
            dialogTitle,
            isPresented: Binding(get: { preview != nil }, set: { if !$0 { onCancelPreview() } }),
            titleVisibility: .visible
        ) {
            if preview != nil {
                Button("确认（\(preview?.cost ?? "")）") { onConfirmPreview() }
                Button("取消", role: .cancel) { onCancelPreview() }
            }
        } message: {
            if let preview {
                Text(detail(for: preview))
            }
        }
    }

    /// 标题走解析后的枚举，不在这里再写一套三元表达式。
    private var dialogTitle: String {
        guard let preview, let action = RewardAction(rawValue: preview.action) else {
            return "确认操作"
        }
        return "确认\(action.title)"
    }

    private func detail(for preview: RewardPreview) -> String {
        var lines = ["账号：\(preview.nickname)"]
        if RewardAction(rawValue: preview.action) == .redeem {
            // 兑换没有"余额"这个概念，写"实时余额 —"会误导。
            lines.append("实时资格：可兑换")
        } else {
            lines.append("实时余额：\(preview.balanceText)")
        }
        lines.append("预计消耗：\(preview.cost)")
        if let target = preview.target { lines.insert("目标：\(target)", at: 1) }
        lines.append("")
        lines.append("确认后服务端还会再校验一次；结果未定时不会重发。")
        return lines.joined(separator: "\n")
    }

    @ViewBuilder
    private func resultBanner(_ result: RewardActionResult) -> some View {
        let tone: Color = {
            switch result.state {
            case "succeeded": return ConsoleTheme.success
            case "rejected": return ConsoleTheme.secondaryText
            case "pending", "frozen": return ConsoleTheme.warning
            default: return ConsoleTheme.danger
            }
        }()
        HStack(alignment: .firstTextBaseline, spacing: 6) {
            Text(result.message)
                .font(ConsoleType.caption)
                .foregroundStyle(tone)
                .fixedSize(horizontal: false, vertical: true)
            Spacer(minLength: 0)
            Button("知道了") { onDismissResult() }
                .buttonStyle(.plain)
                .font(ConsoleType.caption)
                .foregroundStyle(ConsoleTheme.tertiaryText)
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 6)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(tone.opacity(0.1), in: RoundedRectangle(cornerRadius: 8))
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
