import SwiftUI
import AppKit

#if AICC_VISUAL_TEST
/// 总览页的**「成长活跃」区块**（PR #103）。
///
/// 回答一件事：四个账号今天的热力墙亮了没有，没亮的该去哪个客户端补一次真实任务。
///
/// 三条界面约定，来自实测（`docs/activity-rules-2026-11-17.md`）：
///
/// 1. **签到与活跃分开显示。** 四个账号连续 5 天签到成功，其中三个热力墙仍是 0；
///    把两列合并成"今日已完成"会说错话。
/// 2. **`—` 就是没查到。** `todayActive == nil` 显示 `—` 与"未知"，不显示"未活跃"。
/// 3. **不提供任何"一键活跃"。** 活跃必须由用户在官方客户端产生；AICC 只提醒、
///    只回查。所以这一块唯一的主操作是「打开客户端」和「刷新」。
struct ConsoleActivityBlock: View {
    let fleet: ActivityFleetState
    let loading: Bool
    let failure: String?
    let lastRefresh: Date?
    let onRefresh: () -> Void

    var body: some View {
        ConsoleCard {
            VStack(alignment: .leading, spacing: 10) {
                header
                if let failure { failureRow(failure, hasCache: !fleet.accounts.isEmpty) }
                if fleet.accounts.isEmpty {
                    Text(loading ? "正在读取四账号活跃状态…" : "还没读取。点「刷新」查一次（只读）。")
                        .font(ConsoleType.small)
                        .foregroundStyle(ConsoleTheme.secondaryText)
                } else {
                    table
                    if !fleet.needsAction.isEmpty || delayedCount > 0 { footer }
                }
            }
        }
    }

    // MARK: - 头部

    private var header: some View {
        ConsoleSectionHeader(
            title: "成长活跃",
            detail: "只读 · 签到与活跃是两个计数"
        ) {
            HStack(spacing: 8) {
                if let lastRefresh {
                    Text("上次读取 \(ConsoleFormat.relative(lastRefresh))")
                        .font(ConsoleType.caption)
                        .foregroundStyle(ConsoleTheme.tertiaryText)
                }
                Button(loading ? "读取中…" : "刷新") { onRefresh() }
                    .buttonStyle(ConsoleSecondaryButtonStyle())
                    .disabled(loading)
                    .help("只读查询：今日热力墙、连登天数与下一档进度。不会替你做活跃")
            }
        }
    }

    // MARK: - 表格

    private var table: some View {
        VStack(spacing: 0) {
            HStack(spacing: 10) {
                column("账号", width: 208, alignment: .leading)
                column("状态", width: 84, alignment: .leading)
                column("今日活跃", width: 108, alignment: .leading)
                column("今日签到", width: 126, alignment: .leading)
                column("连登", width: 68, alignment: .leading)
                column("下一档", width: 138, alignment: .leading)
                Spacer(minLength: 0)
                column("", width: 96, alignment: .trailing)
            }
            .padding(.vertical, 6)

            Divider().overlay(ConsoleTheme.border)

            ForEach(Array(fleet.accounts.enumerated()), id: \.element.id) { index, account in
                row(account)
                if index < fleet.accounts.count - 1 {
                    Divider().overlay(ConsoleTheme.border.opacity(0.6))
                }
            }
        }
    }

    private func column(_ text: String, width: CGFloat, alignment: Alignment) -> some View {
        Text(text)
            .font(ConsoleType.caption)
            .foregroundStyle(ConsoleTheme.tertiaryText)
            .frame(width: width, alignment: alignment)
            .lineLimit(1)
    }

    private func row(_ account: ActivityAccountState) -> some View {
        HStack(alignment: .center, spacing: 10) {
            VStack(alignment: .leading, spacing: 2) {
                Text(account.nickname)
                    .font(ConsoleType.rowTitle)
                    .foregroundStyle(ConsoleTheme.text)
                    .lineLimit(1)
                    .truncationMode(.middle)
                Text(account.staleNote ?? account.recentText ?? "没有活跃记录")
                    .font(ConsoleType.caption)
                    .foregroundStyle(account.staleNote == nil ? ConsoleTheme.tertiaryText : ConsoleTheme.warning)
                    .lineLimit(1)
                    .help(account.recentText ?? "")
            }
            .frame(width: 208, alignment: .leading)

            // 短标签进表格，完整句子（含"该怎么处理"）放 tooltip。
            ConsoleBadge(text: account.diagnosis.shortLabel, tint: tone(for: account))
                .frame(width: 84, alignment: .leading)
                .help(account.diagnosis.tooltip)

            Text(account.activeText)
                .font(ConsoleType.body)
                .foregroundStyle(tone(for: account))
                .frame(width: 108, alignment: .leading)
                .lineLimit(1)

            Text(account.checkinText)
                .font(ConsoleType.body)
                .foregroundStyle(account.checkin.isConfirmed ? ConsoleTheme.secondaryText : ConsoleTheme.warning)
                .frame(width: 126, alignment: .leading)
                .lineLimit(1)
                .help(account.checkin.detail)

            Text(account.streakText)
                .font(ConsoleType.body)
                .foregroundStyle(ConsoleTheme.text)
                .frame(width: 68, alignment: .leading)
                .lineLimit(1)

            Text(account.nextTierText)
                .font(ConsoleType.body)
                .foregroundStyle(ConsoleTheme.secondaryText)
                .frame(width: 138, alignment: .leading)
                .lineLimit(1)
                .help(account.tierLine)

            Spacer(minLength: 0)

            if account.diagnosis.needsClient {
                Button("打开客户端") { WorkBuddyClient.open() }
                    .buttonStyle(ConsoleSecondaryButtonStyle())
                    .disabled(!WorkBuddyClient.isInstalled)
                    .frame(width: 96, alignment: .trailing)
                    .help(WorkBuddyClient.isInstalled
                          ? "打开 WorkBuddy 客户端；需要换登录账号请去「AI 账号」页切换。做一次真实任务后回来点「刷新」回查"
                          : "没找到 WorkBuddy 客户端；请先安装再回来点「刷新」")
            } else {
                Text(account.diagnosis.needsAction ? account.diagnosis.shortAction : "")
                    .font(ConsoleType.caption)
                    .foregroundStyle(ConsoleTheme.tertiaryText)
                    .lineLimit(1)
                    .frame(width: 96, alignment: .trailing)
                    .help(account.diagnosis.tooltip)
            }
        }
        .padding(.vertical, 8)
    }

    // MARK: - 页脚与提示

    private var footer: some View {
        VStack(alignment: .leading, spacing: 4) {
            if !fleet.needsAction.isEmpty {
                Text("有 \(fleet.needsAction.count) 个账号需要你处理。")
                    .font(ConsoleType.small)
                    .foregroundStyle(ConsoleTheme.text)
                ForEach(fleet.needsAction) { account in
                    Text("\(account.nickname)：\(account.diagnosis.action)")
                        .font(ConsoleType.caption)
                        .foregroundStyle(ConsoleTheme.secondaryText)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
            if !fleet.delayed.isEmpty {
                Text("另有 \(fleet.delayed.count) 个账号服务端已有分数但还没标记活跃，过一会儿刷新即可。")
                    .font(ConsoleType.caption)
                    .foregroundStyle(ConsoleTheme.warning)
                    .fixedSize(horizontal: false, vertical: true)
            }
            Text("签到只记账，不点亮热力墙；活跃要在 WorkBuddy 客户端里真实使用一次。客户端登录的是哪个账号，看「AI 账号」页顶部那一行。")
                .font(ConsoleType.caption)
                .foregroundStyle(ConsoleTheme.tertiaryText)
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    private var delayedCount: Int { fleet.delayed.count }

    private func failureRow(_ text: String, hasCache: Bool) -> some View {
        ConsoleInlineNotice(
            kind: .warning,
            message: hasCache ? "这次没读到（下面可能是上次的结果）：\(text)" : text
        )
    }

    private func tone(for account: ActivityAccountState) -> Color {
        switch account.diagnosis.code {
        case "active": return ConsoleTheme.success
        case "checkin_only": return ConsoleTheme.warning
        case "checkin_unverified": return ConsoleTheme.warning
        case "inactive", "needs_auth": return ConsoleTheme.danger
        case "unknown": return ConsoleTheme.secondaryText
        default: return ConsoleTheme.secondaryText
        }
    }
}
#endif

// MARK: - 打开官方客户端

/// 打开本机的 WorkBuddy 客户端。
///
/// 只做"打开"这一件事。换登录账号是「AI 账号」页的能力
/// （`workbuddy.client.switch`，见 `docs/WORKBUDDY_CLIENT_SWITCH.md`），
/// 不在这一排按钮里悄悄做：切换会重启客户端，必须由用户显式确认。
///
/// 找不到客户端时按钮禁用并给出说明 —— 比点了没反应好。
enum WorkBuddyClient {
    static var appURL: URL? {
        let candidates = [
            "/Applications/WorkBuddy.app",
            NSHomeDirectory() + "/Applications/WorkBuddy.app",
            "/Applications/CodeBuddy.app",
            NSHomeDirectory() + "/Applications/CodeBuddy.app",
        ]
        return candidates
            .first { FileManager.default.fileExists(atPath: $0) }
            .map { URL(fileURLWithPath: $0) }
    }

    static var isInstalled: Bool { appURL != nil }

    static func open() {
        guard let url = appURL else { return }
        NSWorkspace.shared.openApplication(at: url, configuration: NSWorkspace.OpenConfiguration())
    }
}
