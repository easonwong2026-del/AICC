import SwiftUI

/// 签到任务页。
///
/// 本阶段只做**展示**：任务清单与账号对应关系来自 PRD，执行状态来自账号库
/// （`checkin_enabled`）。没有执行记录就是没有，不伪造"已签到"。
struct ConsoleCheckinView: View {
    @ObservedObject var store: ConsoleStore

    private struct TaskDefinition: Identifiable {
        let id: String
        let title: String
        let detail: String
        let symbol: String
        let platform: String
    }

    private let tasks: [TaskDefinition] = [
        .init(id: "wb-daily", title: "WorkBuddy 每日签到", detail: "查询当日状态并领取",
              symbol: "calendar.badge.checkmark", platform: "WorkBuddy"),
        .init(id: "wb-growth", title: "成长中心任务", detail: "任务奖励、盲盒与抽奖",
              symbol: "sparkles", platform: "WorkBuddy"),
        .init(id: "wb-travel", title: "Buddy 旅行", detail: "派出、返回与礼物领取",
              symbol: "airplane", platform: "WorkBuddy"),
        .init(id: "qoder-credits", title: "Qoder CN 每日 Credits", detail: "每天 100 Credits，10:00 刷新",
              symbol: "circle.dotted.circle", platform: "Qoder CN"),
    ]

    @State private var platformFilter: ConsolePlatformFilter = .all

    var body: some View {
        VStack(alignment: .leading, spacing: 18) {
            notice
            taskSection
            accountSection
        }
    }

    private var notice: some View {
        HStack(spacing: 9) {
            Image(systemName: "info.circle")
                .font(.system(size: 11))
            Text("签到与领取尚未接入：本页不会发送任何请求，也不会显示虚构的执行结果。接入后由调度器按北京时间执行。")
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
            ConsoleSectionTitle(title: "任务", detail: "共 \(tasks.count) 项 · 全部待接入")
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

            ConsoleBadge(text: "待接入", tint: ConsoleTheme.tertiaryText)
                .frame(width: 56, alignment: .trailing)

            Button("立即执行") {}
                .buttonStyle(ConsoleSecondaryButtonStyle())
                .disabled(true)
                .help("PR1b 联调后开放")
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 11)
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
        store.accounts.filter { platformFilter.includes($0) }
    }

    private func accountRow(_ account: ConsoleAccount) -> some View {
        HStack(spacing: 10) {
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
            Text(account.checkinEnabled ? "签到已开启" : "签到未开启")
                .font(.system(size: 10))
                .foregroundStyle(ConsoleTheme.secondaryText)
            Text("暂无执行记录")
                .font(.system(size: 10))
                .foregroundStyle(ConsoleTheme.tertiaryText)
                .frame(width: 92, alignment: .trailing)
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 10)
    }
}
