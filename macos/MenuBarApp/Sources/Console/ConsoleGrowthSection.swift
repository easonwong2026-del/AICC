import SwiftUI

/// 签到任务页里的 **成长中心** 区域（PR #67：只读）。
///
/// 这个区域只回答"现在有什么任务、进度多少、能领多少"，
/// **不会接受任务、不会领奖、不会补签、不会抽奖** —— 那些在 #68/#69，
/// 每个都会有自己的开关。所以这里连一个"执行"按钮都没有，
/// 免得给出一个点了没反应的假按钮。
///
/// 扫描是用户点出来的：一次要打 9 个接口 × N 个账号，不能偷偷跑。
struct ConsoleGrowthSection: View {
    @ObservedObject var store: ConsoleStore

    private var result: GrowthScanResult { store.growthResult }

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            header
            card
            footerNote
        }
    }

    // MARK: - 头部

    private var header: some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            ConsoleSectionTitle(title: "成长中心", detail: detailText)
            Spacer(minLength: 8)
            if result.overview.claimable > 0 {
                ConsoleBadge(
                    text: "\(result.overview.claimable) 项待领奖",
                    tint: ConsoleTheme.warning
                )
            }
            if result.overview.needsAuth > 0 {
                ConsoleBadge(
                    text: "\(result.overview.needsAuth) 个待授权",
                    tint: ConsoleTheme.danger
                )
            }
            Button {
                Task { await store.loadGrowth() }
            } label: {
                Image(systemName: "arrow.triangle.2.circlepath")
                    .font(.system(size: 11, weight: .medium))
            }
            .buttonStyle(ConsoleIconButtonStyle())
            .disabled(store.growthLoading)
            .help("扫描一次成长中心（只读，不会执行任何任务）")

            Button("扫描") {
                Task { await store.loadGrowth() }
            }
            .buttonStyle(ConsoleSecondaryButtonStyle())
            .disabled(store.growthLoading)
        }
    }

    // MARK: - 自动管理开关

    /// 整块区域只有一个开关。开着才可能有写操作；关着连 tick 都不发请求。
    private var automationBar: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 10) {
                Toggle(
                    "成长中心自动管理",
                    isOn: Binding(
                        get: { store.growthAuto.enabled },
                        set: { enabled in
                            Task { await store.setGrowthAutoEnabled(enabled) }
                        }
                    )
                )
                .toggleStyle(.switch)
                .controlSize(.small)
                .font(.system(size: 12, weight: .medium))
                .disabled(store.growthActing)

                Spacer(minLength: 8)

                if store.growthAuto.enabled {
                    Text(store.growthAuto.intervalText)
                        .font(.system(size: 10))
                        .foregroundStyle(ConsoleTheme.tertiaryText)
                    Text(store.growthAuto.nextTickText)
                        .font(.system(size: 10))
                        .foregroundStyle(ConsoleTheme.tertiaryText)
                }

                Button("立即检查") {
                    Task { await store.runGrowthTick(force: true) }
                }
                .buttonStyle(ConsoleSecondaryButtonStyle())
                .disabled(store.growthActing || store.growthAuto.running)
                .help("跑一轮：接单 + 领奖。开关关着时只做只读检查")
            }

            if let summary = store.growthAuto.lastResult, store.growthAuto.enabled {
                Text("最近：\(summary)")
                    .font(.system(size: 10))
                    .foregroundStyle(ConsoleTheme.secondaryText)
            }
            if let notice = store.growthNotice {
                Text(notice)
                    .font(.system(size: 10))
                    .foregroundStyle(ConsoleTheme.secondaryText)
            }
            if !store.growthAuto.enabled {
                Text("开启后 AICC 会替你接单、并在任务完成后自动领奖；不代替你完成任何任务。")
                    .font(.system(size: 10))
                    .foregroundStyle(ConsoleTheme.tertiaryText)
            }
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 10)
    }

    private var detailText: String {
        if store.growthLoading { return "扫描中…" }
        if let error = store.growthError { return error }
        let mode = store.growthAuto.enabled ? "自动管理已开启" : "只读"
        guard let scanned = store.lastGrowthScan else { return "尚未扫描 · \(mode)" }
        let time = Self.timeFormatter.string(from: scanned)
        return "\(result.overview.headline) · \(time) 扫描 · \(mode)"
    }

    // MARK: - 卡片

    private var card: some View {
        ConsoleCard(padding: 0) {
            if result.scans.isEmpty {
                emptyState
            } else {
                VStack(spacing: 0) {
                    automationBar
                    Divider().overlay(ConsoleTheme.border)
                    ForEach(Array(result.scans.enumerated()), id: \.element.id) { index, scan in
                        accountBlock(scan)
                        if index < result.scans.count - 1 {
                            Divider().overlay(ConsoleTheme.border).padding(.leading, 16)
                        }
                    }
                }
            }
        }
    }

    @ViewBuilder
    private var emptyState: some View {
        if let error = store.growthError {
            ConsoleEmptyState(
                symbol: "exclamationmark.triangle",
                title: "扫描失败",
                message: error
            )
        } else if store.growthLoading {
            ConsoleEmptyState(
                symbol: "hourglass",
                title: "正在扫描",
                message: "逐个账号查询任务、积分、能量与连签状态。"
            )
        } else {
            ConsoleEmptyState(
                symbol: "sparkles",
                title: "还没扫描过",
                message: "点右上角「扫描」拉一次成长中心。这一步只读：不会接受任务，也不会领取奖励。"
            )
        }
    }

    // MARK: - 账号行

    private func accountBlock(_ scan: GrowthAccountScan) -> some View {
        let expanded = store.expandedGrowthAccount == scan.accountId
        return VStack(alignment: .leading, spacing: 0) {
            Button {
                store.toggleGrowthAccount(scan.accountId)
            } label: {
                HStack(spacing: 10) {
                    Image(systemName: expanded ? "chevron.down" : "chevron.right")
                        .font(.system(size: 9, weight: .semibold))
                        .foregroundStyle(ConsoleTheme.tertiaryText)
                        .frame(width: 10)

                    VStack(alignment: .leading, spacing: 2) {
                        HStack(spacing: 6) {
                            Text(scan.nickname)
                                .font(.system(size: 12, weight: .medium))
                                .foregroundStyle(ConsoleTheme.text)
                                .lineLimit(1)
                            if let source = scan.authSourceText {
                                ConsoleBadge(text: source, tint: ConsoleTheme.tertiaryText)
                            }
                        }
                        Text(scan.profile.summaryText)
                            .font(.system(size: 10))
                            .foregroundStyle(ConsoleTheme.tertiaryText)
                            .lineLimit(1)
                    }
                    .frame(width: 236, alignment: .leading)

                    ConsoleBadge(text: scan.stateText, tint: tone(for: scan))
                        .frame(width: 92, alignment: .leading)

                    Text(countsText(scan))
                        .font(.system(size: 11))
                        .foregroundStyle(ConsoleTheme.secondaryText)
                        .lineLimit(1)

                    Spacer(minLength: 8)

                    if let runs = store.growthRuns[scan.accountId], runs.grantedCredit > 0 {
                        Text("已领 +\(runs.grantedCredit)")
                            .font(.system(size: 10))
                            .foregroundStyle(ConsoleTheme.success)
                    }

                    if let travel = scan.travel {
                        Text(travel.stateText)
                            .font(.system(size: 10))
                            .foregroundStyle(ConsoleTheme.tertiaryText)
                    }
                }
                .padding(.horizontal, 16)
                .padding(.vertical, 10)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)

            if expanded {
                taskList(scan)
            }
        }
    }

    private func countsText(_ scan: GrowthAccountScan) -> String {
        if scan.needsAuth { return "需要重新授权" }
        var parts: [String] = ["\(scan.counts.total) 个任务"]
        if scan.counts.claimable > 0 { parts.append("\(scan.counts.claimable) 待领奖") }
        if scan.counts.inProgress > 0 { parts.append("\(scan.counts.inProgress) 进行中") }
        if scan.counts.acceptable > 0 { parts.append("\(scan.counts.acceptable) 可接受") }
        if scan.counts.manual > 0 { parts.append("\(scan.counts.manual) 需人工") }
        if scan.counts.unknown > 0 { parts.append("\(scan.counts.unknown) 待识别") }
        return parts.joined(separator: " · ")
    }

    // MARK: - 任务列表

    @ViewBuilder
    private func taskList(_ scan: GrowthAccountScan) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            if scan.tasks.isEmpty {
                Text(scan.needsAuth
                     ? "这个账号没有可用授权，先到「AI 账号」页重新登录或从 Magpie 导入。"
                     : "服务端没有下发任务。")
                    .font(.system(size: 10))
                    .foregroundStyle(ConsoleTheme.tertiaryText)
                    .padding(.horizontal, 44)
                    .padding(.bottom, 10)
            } else {
                ForEach(scan.tasks) { task in
                    taskRow(task, accountId: scan.accountId)
                }
            }

            ForEach(scan.errors) { error in
                Text("\(error.field)：\(error.message)")
                    .font(.system(size: 10))
                    .foregroundStyle(error.needsUserAction ? ConsoleTheme.danger : ConsoleTheme.tertiaryText)
                    .padding(.horizontal, 44)
                    .padding(.bottom, 3)
            }
        }
        .padding(.bottom, 8)
        .background(ConsoleTheme.cardSubtle)
    }

    private func taskRow(_ task: GrowthTaskRow, accountId: String) -> some View {
        HStack(alignment: .top, spacing: 10) {
            Image(systemName: symbol(for: task.taskClass))
                .font(.system(size: 10, weight: .medium))
                .foregroundStyle(ConsoleTheme.accent)
                .frame(width: 16)
                .padding(.top, 1)

            VStack(alignment: .leading, spacing: 2) {
                Text(task.title)
                    .font(.system(size: 11, weight: .medium))
                    .foregroundStyle(ConsoleTheme.text)
                    .lineLimit(1)
                Text(task.condition)
                    .font(.system(size: 10))
                    .foregroundStyle(ConsoleTheme.tertiaryText)
                    .lineLimit(2)
                    .fixedSize(horizontal: false, vertical: true)
                if let record = store.growthRuns[accountId]?.latestByTask[task.code] {
                    Text("\(record.summary) · \(record.timeText)")
                        .font(.system(size: 9))
                        .foregroundStyle(record.outcome.isProblem ? ConsoleTheme.danger : ConsoleTheme.tertiaryText)
                        .lineLimit(1)
                }
            }

            Spacer(minLength: 8)

            Text(task.progressText)
                .font(.system(size: 10, design: .monospaced))
                .foregroundStyle(ConsoleTheme.secondaryText)
                .frame(width: 54, alignment: .trailing)

            Text(task.rewardText)
                .font(.system(size: 10))
                .foregroundStyle(ConsoleTheme.secondaryText)
                .frame(width: 108, alignment: .trailing)

            ConsoleBadge(
                text: task.status.title,
                tint: task.status.needsAttention ? ConsoleTheme.warning : ConsoleTheme.tertiaryText
            )
            .frame(width: 62, alignment: .leading)

            // 类型必须显式展示：未知类型不会被自动执行，得让人看得见。
            ConsoleBadge(
                text: task.taskClass.title,
                tint: task.isUnrecognised ? ConsoleTheme.danger : ConsoleTheme.tertiaryText
            )
            .frame(width: 62, alignment: .leading)
            .help(task.classReason)
        }
        .padding(.leading, 44)
        .padding(.trailing, 16)
        .padding(.vertical, 6)
    }

    private func symbol(for taskClass: GrowthTaskClass) -> String {
        switch taskClass {
        case .automatic: return "gearshape.2"
        case .realAction: return "wrench.and.screwdriver"
        case .manual: return "person.crop.circle.badge.exclamationmark"
        case .unknown: return "questionmark.circle"
        }
    }

    private func tone(for scan: GrowthAccountScan) -> Color {
        switch scan.state {
        case "ok": return ConsoleTheme.success
        case "partial": return ConsoleTheme.warning
        case "unauthorized", "missing_credential": return ConsoleTheme.danger
        case "failed": return ConsoleTheme.danger
        default: return ConsoleTheme.tertiaryText
        }
    }

    // MARK: - 底部说明

    private var footerNote: some View {
        VStack(alignment: .leading, spacing: 3) {
            Text(footerLine)
                .font(.system(size: 10))
                .foregroundStyle(ConsoleTheme.secondaryText)
                .fixedSize(horizontal: false, vertical: true)
            if let prefix = result.scans.compactMap({ $0.pathPrefix }).first {
                // 线上排查用：两种路径形态到底哪个生效，直接写在界面上。
                Text("接口前缀 \(prefix) · 每个账号 9 个只读请求")
                    .font(.system(size: 10))
                    .foregroundStyle(ConsoleTheme.tertiaryText)
            }
        }
    }

    /// 必须让用户一眼看清"现在到底会不会动服务端"。
    private var footerLine: String {
        if store.growthAuto.enabled {
            return "自动管理已开启：只做接单与领奖，不代替你完成任何任务。补签、兑换、抽奖、开盲盒都不会自动执行。"
        }
        return "当前只读：不会接受任务、不会领取奖励。开启自动管理后，AICC 只做接单与领奖，不代替你完成任何任务。"
    }

    private static let timeFormatter: DateFormatter = {
        let formatter = DateFormatter()
        formatter.dateFormat = "HH:mm"
        return formatter
    }()
}
