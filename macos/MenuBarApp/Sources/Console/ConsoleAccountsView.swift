import AppKit
import SwiftUI

struct ConsoleAccountsView: View {
    @ObservedObject var store: ConsoleStore

    @State private var filter: ConsolePlatformFilter = .all
    @State private var query = ""

    private var visibleAccounts: [ConsoleAccount] {
        let trimmed = query.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        return store.accounts.filter { account in
            filter.includes(account) && (trimmed.isEmpty || account.searchText.contains(trimmed))
        }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            toolbar
            if let error = store.accountsError {
                errorBanner(error)
            }
            content
        }
    }

    private var toolbar: some View {
        HStack(spacing: 10) {
            HStack(spacing: 6) {
                Image(systemName: "magnifyingglass")
                    .font(.system(size: 11))
                    .foregroundStyle(ConsoleTheme.tertiaryText)
                TextField("搜索昵称或 UID", text: $query)
                    .textFieldStyle(.plain)
                    .font(.system(size: 12))
                    .foregroundStyle(ConsoleTheme.text)
                if !query.isEmpty {
                    Button {
                        query = ""
                    } label: {
                        Image(systemName: "xmark.circle.fill")
                            .font(.system(size: 11))
                            .foregroundStyle(ConsoleTheme.tertiaryText)
                    }
                    .buttonStyle(.plain)
                }
            }
            .padding(.horizontal, 9)
            .frame(width: 200, height: 26)
            .background(ConsoleTheme.card, in: RoundedRectangle(cornerRadius: 7))
            .overlay(
                RoundedRectangle(cornerRadius: 7).strokeBorder(ConsoleTheme.border, lineWidth: 1)
            )

            Picker("平台", selection: $filter) {
                ForEach(ConsolePlatformFilter.allCases) { option in
                    Text(option.title).tag(option)
                }
            }
            .pickerStyle(.segmented)
            .labelsHidden()
            .tint(ConsoleTheme.accent)
            .frame(width: 240)

            Spacer(minLength: 8)

            Text("\(visibleAccounts.count) / \(store.accounts.count) 个账号")
                .font(.system(size: 11))
                .foregroundStyle(ConsoleTheme.secondaryText)

            Button("添加账号") {}
                .buttonStyle(ConsolePrimaryButtonStyle(enabled: false))
                .disabled(true)
                .help("扫码登录在 PR1b 完成真实账号验证后开放")
        }
    }

    private func errorBanner(_ message: String) -> some View {
        HStack(spacing: 8) {
            Image(systemName: "exclamationmark.triangle")
                .font(.system(size: 11))
            Text(message)
                .font(.system(size: 11))
                .fixedSize(horizontal: false, vertical: true)
            Spacer(minLength: 8)
            Button("重试") { Task { await store.refreshAccounts() } }
                .buttonStyle(.borderless)
                .font(.system(size: 11, weight: .medium))
        }
        .foregroundStyle(ConsoleTheme.warning)
        .padding(.horizontal, 12)
        .padding(.vertical, 9)
        .background(ConsoleTheme.warning.opacity(0.10), in: RoundedRectangle(cornerRadius: 9))
    }

    @ViewBuilder
    private var content: some View {
        if store.accounts.isEmpty {
            ConsoleCard {
                ConsoleEmptyState(
                    symbol: "person.crop.rectangle.stack",
                    title: "还没有添加账号",
                    message: store.accountsError == nil
                        ? "账号库当前为空。扫码登录与多账号导入在 PR1b 接入后开放，现在不会显示任何演示数据。"
                        : "账号库读取失败，先处理上面的错误再重试。",
                    actionTitle: "重新读取",
                    action: { Task { await store.refreshAccounts() } }
                )
            }
        } else if visibleAccounts.isEmpty {
            ConsoleCard {
                ConsoleEmptyState(
                    symbol: "line.3.horizontal.decrease.circle",
                    title: "没有匹配的账号",
                    message: "换个关键词，或把平台筛选切回「全部」。"
                )
            }
        } else {
            LazyVGrid(
                columns: [GridItem(.adaptive(minimum: 300), spacing: ConsoleTheme.cardSpacing)],
                spacing: ConsoleTheme.cardSpacing
            ) {
                ForEach(visibleAccounts) { account in
                    ConsoleAccountCard(account: account)
                }
            }
        }
    }
}

private struct ConsoleAccountCard: View {
    let account: ConsoleAccount

    var body: some View {
        ConsoleCard {
            VStack(alignment: .leading, spacing: 12) {
                HStack(alignment: .top, spacing: 10) {
                    Image(systemName: account.symbolName)
                        .font(.system(size: 12, weight: .semibold))
                        .foregroundStyle(ConsoleTheme.accent)
                        .frame(width: 28, height: 28)
                        .background(ConsoleTheme.accentSoft, in: RoundedRectangle(cornerRadius: 8))

                    VStack(alignment: .leading, spacing: 3) {
                        Text(account.nickname)
                            .font(.system(size: 13, weight: .semibold))
                            .foregroundStyle(ConsoleTheme.text)
                            .lineLimit(1)
                        Text(account.platformName)
                            .font(.system(size: 10))
                            .foregroundStyle(ConsoleTheme.secondaryText)
                    }

                    Spacer(minLength: 6)

                    ConsoleBadge(
                        text: account.authLabel,
                        tint: ConsoleTheme.tone(account.authTone)
                    )
                }

                HStack(spacing: 6) {
                    Text("UID")
                        .font(.system(size: 10))
                        .foregroundStyle(ConsoleTheme.tertiaryText)
                    Text(account.uid)
                        .font(.system(size: 10, design: .monospaced))
                        .foregroundStyle(ConsoleTheme.secondaryText)
                        .lineLimit(1)
                        .truncationMode(.middle)
                        .textSelection(.enabled)
                }

                Divider().overlay(ConsoleTheme.border)

                HStack(spacing: 14) {
                    labelValue("签到", account.checkinEnabled ? "已开启" : "未开启")
                    labelValue("最近更新", shortTimestamp(account.updatedAt))
                    Spacer(minLength: 0)
                }
            }
        }
        .contextMenu {
            Button("复制 UID") {
                NSPasteboard.general.clearContents()
                NSPasteboard.general.setString(account.uid, forType: .string)
            }
            Button("修改备注") {}.disabled(true)
            Button("删除账号") {}.disabled(true)
        }
    }

    private func labelValue(_ label: String, _ value: String) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(label)
                .font(.system(size: 10))
                .foregroundStyle(ConsoleTheme.tertiaryText)
            Text(value)
                .font(.system(size: 11))
                .foregroundStyle(ConsoleTheme.text)
                .lineLimit(1)
        }
    }

    /// `2026-11-09T14:22:31Z` → `11-09 14:22`（本地时区）。
    private func shortTimestamp(_ value: String?) -> String {
        guard let value, !value.isEmpty else { return "—" }
        let parser = ISO8601DateFormatter()
        parser.formatOptions = [.withInternetDateTime]
        guard let date = parser.date(from: value) else { return String(value.prefix(10)) }
        let formatter = DateFormatter()
        formatter.locale = .current
        formatter.dateFormat = "MM-dd HH:mm"
        return formatter.string(from: date)
    }
}
