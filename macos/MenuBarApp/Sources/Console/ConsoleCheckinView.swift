import SwiftUI

/// 签到任务页。
///
/// 三块已接入的能力，各自独立开关，互不牵连：
///
/// | 区域 | 状态 | 会不会写服务端 |
/// | --- | --- | --- |
/// | WorkBuddy 每日签到 | PR #69 接入 | 只在你点按钮或开了自动开关后写，且先查状态 |
/// | 成长中心 | #67 只读扫描 / #68 自动接单与领奖 | 开关关着时只读 |
/// | Buddy 旅行 | #64 接入 | 是你点按钮或开自动开关后才会请求 |
///
/// 没有执行记录就是没有，不伪造"已签到"。
struct ConsoleCheckinView: View {
    @ObservedObject var store: ConsoleStore

    private struct TaskDefinition: Identifiable {
        let id: String
        let title: String
        let detail: String
        let symbol: String
        let platform: String
        /// 已经接线的任务才能点；没接线的按钮保持置灰，不假装能跑。
        var wired: Bool = false
    }

    private let tasks: [TaskDefinition] = [
        .init(id: "wb-daily", title: "WorkBuddy 每日签到", detail: "查询当日状态并领取，多账号独立开关",
              symbol: "calendar.badge.checkmark", platform: "WorkBuddy", wired: true),
        .init(id: "wb-growth", title: "成长中心任务", detail: "任务、积分、能量与连签，可自动接单与领奖",
              symbol: "sparkles", platform: "WorkBuddy", wired: true),
        .init(id: "wb-travel", title: "Buddy 旅行", detail: "派出、到达与奖励领取",
              symbol: "airplane", platform: "WorkBuddy", wired: true),
        .init(id: "qoder-credits", title: "Qoder CN 每日 Credits", detail: "每天 100 Credits，10:00 刷新",
              symbol: "circle.dotted.circle", platform: "Qoder CN"),
    ]

    @State private var platformFilter: ConsolePlatformFilter = .all

    private var wiredCount: Int { tasks.filter(\.wired).count }

    var body: some View {
        VStack(alignment: .leading, spacing: 18) {
            notice
            taskSection
            ConsoleCheckinSection(store: store)
            ConsoleGrowthSection(store: store)
            ConsoleTravelSection(store: store)
            accountSection
        }
        .task {
            // 首屏只读本地记录：不联网、不提交任何东西。
            if store.checkin.rows.isEmpty {
                await store.loadCheckin(cached: true)
            }
        }
    }

    private var notice: some View {
        HStack(spacing: 9) {
            Image(systemName: "info.circle")
                .font(.system(size: 11))
            Text("WorkBuddy 每日签到已接入：每个账号一个开关，默认关闭；每天 09:00（北京时间）后检查一次，服务端已签到就跳过。成长中心可自动接单与领奖，Buddy 旅行按开关执行。所有写操作都只在 AICC 运行时发生。WorkBuddy AI（国际版）没有这几项任务，不会出现在这里。")
                .font(.system(size: 11))
                .fixedSize(horizontal: false, vertical: true)
            Spacer(minLength: 0)
        }
        .foregroundStyle(ConsoleTheme.secondaryText)
        .padding(.horizontal, 12)
        .padding(.vertical, 10)
        .background(ConsoleTheme.cardSubtle, in: RoundedRectangle(cornerRadius: 9))
        .overlay(
            RoundedRectangle(cornerRadius: 9).strokeBorder(ConsoleTheme.border, lineWidth: 1)
        )
    }

    private var taskSection: some View {
        VStack(alignment: .leading, spacing: 10) {
            ConsoleSectionTitle(title: "任务", detail: "共 \(tasks.count) 项 · \(wiredCount) 项已接入")
            ConsoleCard(padding: 0) {
                VStack(spacing: 0) {
                    ForEach(Array(tasks.enumerated()), id: \.element.id) { index, task in
                        taskRow(task)
                        if index < tasks.count - 1 {
                            Divider().overlay(ConsoleTheme.border).padding(.leading, 16)
                        }
                    }
                }
            }
        }
    }

    private func taskRow(_ task: TaskDefinition) -> some View {
        HStack(spacing: 12) {
            Image(systemName: task.symbol)
                .font(.system(size: 12, weight: .medium))
                .foregroundStyle(ConsoleTheme.accent)
                .frame(width: 26, height: 26)
                .background(ConsoleTheme.accentSoft, in: RoundedRectangle(cornerRadius: 7))

            VStack(alignment: .leading, spacing: 2) {
                Text(task.title)
                    .font(.system(size: 12, weight: .medium))
                    .foregroundStyle(ConsoleTheme.text)
                Text(task.detail)
                    .font(.system(size: 10))
                    .foregroundStyle(ConsoleTheme.secondaryText)
            }

            Spacer(minLength: 8)

            Text(task.platform)
                .font(.system(size: 10))
                .foregroundStyle(ConsoleTheme.tertiaryText)
                .frame(width: 78, alignment: .trailing)

            ConsoleBadge(
                text: task.wired ? "已接入" : "待接入",
                tint: task.wired ? ConsoleTheme.accent : ConsoleTheme.tertiaryText
            )
            .frame(width: 56, alignment: .trailing)

            Button(task.wired ? "查看" : "立即执行") {
                Task {
                    // 已接入的任务各自对应下方一块区域。签到与成长中心都要联网，
                    // 所以点「查看」触发的是对应区域的读取动作，而不是假按钮。
                    switch task.id {
                    case "wb-daily":
                        await store.refreshCheckinStatus()
                    case "wb-growth":
                        await store.loadGrowth()
                    default:
                        await store.loadTravel()
                    }
                }
            }
            .buttonStyle(ConsoleSecondaryButtonStyle())
            .disabled(!task.wired)
            .help(taskHelp(task))
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 11)
    }

    private func taskHelp(_ task: TaskDefinition) -> String {
        switch task.id {
        case "wb-daily": return "向服务端查一次今日签到状态（只读，不会签到）"
        case "wb-growth": return "扫描成长中心（只读）"
        case "wb-travel": return "刷新下方的 Buddy 旅行区域"
        default: return "后续版本接入"
        }
    }

    private var accountSection: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack {
                ConsoleSectionTitle(title: "按账号查看", detail: "\(filteredAccounts.count) 个账号")
                Picker("平台", selection: $platformFilter) {
                    ForEach(ConsolePlatformFilter.allCases) { option in
                        Text(option.title).tag(option)
                    }
                }
                .pickerStyle(.segmented)
                .labelsHidden()
                .tint(ConsoleTheme.accent)
                .frame(width: 240)
            }

            ConsoleCard(padding: 0) {
                if filteredAccounts.isEmpty {
                    ConsoleEmptyState(
                        symbol: "person.crop.rectangle.stack",
                        title: "没有可展示的账号",
                        message: "账号库为空时无法按账号查看任务状态。先到「AI 账号」页确认账号。"
                    )
                } else {
                    VStack(spacing: 0) {
                        ForEach(Array(filteredAccounts.enumerated()), id: \.element.id) { index, account in
                            accountRow(account)
                            if index < filteredAccounts.count - 1 {
                                Divider().overlay(ConsoleTheme.border).padding(.leading, 16)
                            }
                        }
                    }
                }
            }
        }
    }

    private var filteredAccounts: [ConsoleAccount] {
        // 国际版没有签到 / 成长 / 旅行这些任务，不进这一页。
        store.accounts.filter { platformFilter.includes($0) && $0.supportsCNTasks }
    }

    private func accountRow(_ account: ConsoleAccount) -> some View {
        // 签到那一列优先用真实结论；本地没记录就说"暂无记录"，不写"已签到"。
        let checkinRow = store.checkin.row(account.id)
        return HStack(spacing: 10) {
            Image(systemName: account.symbolName)
                .font(.system(size: 11))
                .foregroundStyle(ConsoleTheme.secondaryText)
                .frame(width: 18)
            Text(account.nickname)
                .font(.system(size: 12))
                .foregroundStyle(ConsoleTheme.text)
                .lineLimit(1)
            Text(account.platformName)
                .font(.system(size: 10))
                .foregroundStyle(ConsoleTheme.tertiaryText)
            Spacer(minLength: 8)
            Text(checkinRow?.state.title ?? "暂无记录")
                .font(.system(size: 10))
                .foregroundStyle(ConsoleTheme.secondaryText)
            Text(checkinRow == nil ? "—" : checkinRow!.creditsText)
                .font(.system(size: 10))
                .monospacedDigit()
                .foregroundStyle(ConsoleTheme.tertiaryText)
                .frame(width: 52, alignment: .trailing)
            Text(account.checkinEnabled ? "自动签到已开" : "自动签到未开")
                .font(.system(size: 10))
                .foregroundStyle(ConsoleTheme.secondaryText)
                .frame(width: 96, alignment: .trailing)
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 10)
    }
}
