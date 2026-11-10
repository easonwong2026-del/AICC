import SwiftUI

/// 从 Magpie 借用 WorkBuddy 登录态。
///
/// 只复制 access token：Magpie 的 refresh token 是**单份有状态凭据**，
/// 两边各自刷新会互相踢下线。AICC 这边不刷新，过期后重新同步一次即可。
struct MagpieImportSheet: View {
    @ObservedObject var store: ConsoleStore
    @Environment(\.dismiss) private var dismiss

    @State private var preview: MagpiePreview?
    @State private var loading = true
    @State private var importing = false
    @State private var message: String?
    @State private var failed = false

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            header

            Group {
                if loading {
                    HStack(spacing: 8) {
                        ProgressView().controlSize(.small)
                        Text("正在读取 Magpie 的登录数据…")
                            .font(.system(size: 12))
                            .foregroundStyle(ConsoleTheme.secondaryText)
                    }
                    .frame(maxWidth: .infinity, minHeight: 120)
                } else if let preview, preview.available {
                    accountList(preview)
                } else {
                    unavailable
                }
            }

            if let message {
                Text(message)
                    .font(.system(size: 11))
                    .foregroundStyle(failed ? ConsoleTheme.danger : ConsoleTheme.secondaryText)
                    .fixedSize(horizontal: false, vertical: true)
            }

            footer
        }
        .padding(20)
        .frame(width: 470)
        .background(ConsoleTheme.canvas)
        .task { await load() }
    }

    private var header: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text("从 Magpie 导入账号")
                .font(.system(size: 15, weight: .semibold))
                .foregroundStyle(ConsoleTheme.text)
            Text("只复制访问令牌，不用 Magpie 的刷新令牌 —— 两边各自刷新会互相把对方踢下线，所以 AICC 这里不刷新，过期后重新同步一次即可。")
                .font(.system(size: 11))
                .foregroundStyle(ConsoleTheme.secondaryText)
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    private func accountList(_ preview: MagpiePreview) -> some View {
        VStack(spacing: 0) {
            ForEach(Array(preview.candidates.enumerated()), id: \.element.id) { index, candidate in
                if index > 0 {
                    Divider().overlay(ConsoleTheme.border)
                }
                row(candidate)
            }
        }
        .background(ConsoleTheme.card, in: RoundedRectangle(cornerRadius: 10))
        .overlay(
            RoundedRectangle(cornerRadius: 10).strokeBorder(ConsoleTheme.border, lineWidth: 1)
        )
    }

    private func row(_ candidate: MagpieAccountCandidate) -> some View {
        HStack(spacing: 10) {
            Image(systemName: candidate.imported ? "checkmark.circle.fill" : "person.crop.circle")
                .font(.system(size: 13))
                .foregroundStyle(candidate.imported ? ConsoleTheme.success : ConsoleTheme.accent)
                .frame(width: 22)

            VStack(alignment: .leading, spacing: 2) {
                Text(candidate.nickname)
                    .font(.system(size: 12, weight: .medium))
                    .foregroundStyle(candidate.expired ? ConsoleTheme.tertiaryText : ConsoleTheme.text)
                    .lineLimit(1)
                Text(candidate.uid)
                    .font(.system(size: 10, design: .monospaced))
                    .foregroundStyle(ConsoleTheme.tertiaryText)
                    .lineLimit(1)
                    .truncationMode(.middle)
            }

            Spacer(minLength: 8)

            Text(candidate.statusText)
                .font(.system(size: 10))
                .foregroundStyle(candidate.expired ? ConsoleTheme.danger : ConsoleTheme.secondaryText)
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 9)
    }

    private var unavailable: some View {
        VStack(alignment: .leading, spacing: 8) {
            Label("没有可导入的账号", systemImage: "questionmark.folder")
                .font(.system(size: 12, weight: .medium))
                .foregroundStyle(ConsoleTheme.text)
            Text(preview?.message ?? "本机没有找到 Magpie 的登录数据。")
                .font(.system(size: 11))
                .foregroundStyle(ConsoleTheme.secondaryText)
                .fixedSize(horizontal: false, vertical: true)
        }
        .padding(14)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(ConsoleTheme.card, in: RoundedRectangle(cornerRadius: 10))
    }

    private var footer: some View {
        HStack(spacing: 10) {
            if let preview, preview.available {
                Text("共 \(preview.candidates.count) 个账号，可导入 \(preview.importable.count) 个")
                    .font(.system(size: 11))
                    .foregroundStyle(ConsoleTheme.tertiaryText)
            }
            Spacer(minLength: 8)
            Button("取消") { dismiss() }
                .buttonStyle(ConsoleSecondaryButtonStyle())
            Button(importing ? "导入中…" : "导入全部") {
                Task { await runImport() }
            }
            .buttonStyle(ConsolePrimaryButtonStyle(
                enabled: !importing && !(preview?.importable.isEmpty ?? true)
            ))
            .disabled(importing || (preview?.importable.isEmpty ?? true))
        }
    }

    private func load() async {
        loading = true
        defer { loading = false }
        do {
            preview = try await store.magpiePreview()
        } catch {
            failed = true
            message = "读取失败：\(error.localizedDescription)"
        }
    }

    /// 逐个用真实接口核一次登录态。失败不影响"已导入"的事实。
    private func verify(keys: [String]) async -> (valid: Int, total: Int, checkedIn: Int)? {
        guard let preview else { return nil }
        let targets = preview.candidates.filter { keys.contains($0.key) && $0.accountId != nil }
        guard !targets.isEmpty else { return nil }

        var valid = 0
        var checkedIn = 0
        for target in targets {
            guard let id = target.accountId,
                  let status = try? await store.workbuddyStatus(accountId: id) else { continue }
            if status["state"] as? String == "valid" {
                valid += 1
                if status["todayCheckedIn"] as? Bool == true { checkedIn += 1 }
            }
        }
        await store.refreshAccounts()
        return (valid, targets.count, checkedIn)
    }

    private func runImport() async {
        guard let preview, !preview.importable.isEmpty else { return }
        importing = true
        message = nil
        failed = false
        defer { importing = false }
        do {
            let keys = preview.importable.map(\.key)
            let result = try await store.importMagpieAccounts(keys: keys)
            await store.refreshAccounts()
            await load()

            // 导入只是把令牌搬过来；再花一次请求确认它真的能用，
            // 否则界面上的"认证有效"只是我们自己写进去的乐观值。
            let verified = await verify(keys: keys)
            var parts = ["已导入 \(result.imported) 个"]
            if result.skipped > 0 { parts.append("跳过 \(result.skipped) 个") }
            if let verified {
                parts.append("登录态验证通过 \(verified.valid)/\(verified.total)")
                if verified.checkedIn > 0 {
                    parts.append("其中 \(verified.checkedIn) 个今日已签到")
                }
            }
            message = parts.joined(separator: "，") + "。"
        } catch {
            failed = true
            message = "导入失败：\(error.localizedDescription)"
        }
    }
}
