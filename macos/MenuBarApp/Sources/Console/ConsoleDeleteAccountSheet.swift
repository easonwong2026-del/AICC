import SwiftUI

/// 删除 Qoder **服务账号**的确认面板（PRD §4.5）。
///
/// 四个后果必须逐条写清楚，因为"删除"在这款 App 里有三个不同的对象：
/// AICC 保存的凭据 / Magpie 配置 / Qoder 客户端本机 Profile。
/// 这个面板只负责第一个。
struct ConsoleDeleteAccountSheet: View {
    @ObservedObject var store: ConsoleStore
    let accountId: String
    let onDeleted: () -> Void

    @Environment(\.dismiss) private var dismiss
    @State private var working = false

    private var account: QoderServiceAccount? {
        store.qoderServiceAccounts.first { $0.id == accountId }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack(spacing: 9) {
                Image(systemName: "trash")
                    .font(.system(size: 13, weight: .semibold))
                    .foregroundStyle(ConsoleTheme.danger)
                    .frame(width: 28, height: 28)
                    .background(ConsoleTheme.dangerSoft, in: RoundedRectangle(cornerRadius: 8))
                VStack(alignment: .leading, spacing: 1) {
                    Text("从 AICC 移除账号")
                        .font(ConsoleType.sectionTitle)
                        .foregroundStyle(ConsoleTheme.text)
                    Text(account.map { "\($0.displayName) · \($0.editionName)" } ?? "该账号已不在列表里")
                        .font(ConsoleType.caption)
                        .foregroundStyle(ConsoleTheme.secondaryText)
                        .lineLimit(1)
                }
                Spacer(minLength: 0)
            }

            VStack(alignment: .leading, spacing: 6) {
                bullet("会删除 AICC 自己保存的该账号凭据、授权元数据、领取记录和额度快照。", tone: .error)
                bullet("不会删除 Magpie 的配置。", tone: .neutral)
                bullet("不会删除 Qoder 客户端的本机 Profile。", tone: .neutral)
                bullet("不代表服务端 PAT 已被撤销；如需撤销，请到 Qoder 官方账户页操作。", tone: .warning)
            }

            if let error = store.qoderDeleteError {
                ConsoleInlineNotice(kind: .error, message: error)
            } else if store.qoderDeleteNotice != nil {
                ConsoleInlineNotice(kind: .info, message: "已移除，列表已刷新。")
            }

            Divider().overlay(ConsoleTheme.border)

            HStack(spacing: 8) {
                Text("删除期间账号仍显示在列表里，直到助手确认完成。")
                    .font(ConsoleType.caption)
                    .foregroundStyle(ConsoleTheme.tertiaryText)
                    .lineLimit(2)
                Spacer(minLength: 8)
                Button("取消") { dismiss() }
                    .buttonStyle(ConsoleSecondaryButtonStyle())
                Button(working ? "移除中…" : (store.qoderDeleteError == nil ? "移除账号" : "重试")) {
                    Task { await remove() }
                }
                .buttonStyle(ConsoleDangerButtonStyle())
                .disabled(working || account == nil)
            }
        }
        .padding(16)
        .frame(width: 460)
        .background(ConsoleTheme.canvas)
        .onAppear { store.clearQoderDeleteFeedback() }
    }

    private func bullet(_ text: String, tone: ConsoleTheme.Tone) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 6) {
            Circle()
                .fill(ConsoleTheme.color(tone))
                .frame(width: 5, height: 5)
                .padding(.top, 5)
            Text(text)
                .font(ConsoleType.small)
                .foregroundStyle(ConsoleTheme.secondaryText)
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    private func remove() async {
        guard let account else { return }
        working = true
        defer { working = false }
        let removed = await store.deleteQoderServiceAccount(account)
        if removed {
            onDeleted()
            // 成功后才关闭：失败留在面板里，错误与重试都在原位。
            dismiss()
        }
    }
}

/// 危险操作的按钮样式：红底白字，只在确认面板里用。
struct ConsoleDangerButtonStyle: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        StyleBody(configuration: configuration)
    }

    private struct StyleBody: View {
        @Environment(\.isEnabled) private var isEnabled
        let configuration: Configuration

        var body: some View {
            configuration.label
                .font(ConsoleType.body)
                .lineLimit(1)
                .foregroundStyle(isEnabled ? Color.white : ConsoleTheme.tertiaryText)
                .padding(.horizontal, 11)
                .frame(height: ConsoleTheme.controlHeight)
                .background(
                    isEnabled
                        ? ConsoleTheme.danger.opacity(configuration.isPressed ? 0.82 : 1)
                        : ConsoleTheme.cardSubtle,
                    in: RoundedRectangle(cornerRadius: 7)
                )
        }
    }
}
