import AppKit
import SwiftUI

struct ConsoleAccountsView: View {
    @ObservedObject var store: ConsoleStore

    @State private var filter: ConsolePlatformFilter = .all
    @State private var query = ""
    @State private var showAdd = false
    @State private var showMagpie = false

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
        .task {
            // QA 钩子：`open --args -AICCOpenAddAccount 1` 直接拉起扫码面板。
            if UserDefaults.standard.bool(forKey: "AICCOpenAddAccount")
                || ProcessInfo.processInfo.environment["AICC_OPEN_ADD_ACCOUNT"] == "1" {
                showAdd = true
            }
            // 同理：`open --args -AICCOpenMagpieImport 1` 直接拉起 Magpie 导入面板。
            if UserDefaults.standard.bool(forKey: "AICCOpenMagpieImport")
                || ProcessInfo.processInfo.environment["AICC_OPEN_MAGPIE_IMPORT"] == "1" {
                showMagpie = true
            }
        }
        .sheet(isPresented: $showAdd) {
            AddWorkbuddyAccountSheet(store: store) {
                Task { await store.refreshAccounts() }
            }
        }
        .sheet(isPresented: $showMagpie) {
            MagpieImportSheet(store: store)
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

            Button("从 Magpie 导入") { showMagpie = true }
                .buttonStyle(ConsoleSecondaryButtonStyle())
                .help("复用 Magpie 已登录的 WorkBuddy 账号（只复制访问令牌）")

            Button("扫码添加") { showAdd = true }
                .buttonStyle(ConsolePrimaryButtonStyle())
                .help("用手机 WorkBuddy 扫码授权")
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
                        ? "账号库当前为空。可以用手机扫码授权，也可以直接复用 Magpie 里已经登录的 WorkBuddy 账号。"
                        : "账号库读取失败，先处理上面的错误再重试。",
                    actionTitle: store.accountsError == nil ? "扫码添加 WorkBuddy 账号" : "重新读取",
                    action: {
                        if store.accountsError == nil {
                            showAdd = true
                        } else {
                            Task { await store.refreshAccounts() }
                        }
                    }
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
                    ConsoleAccountCard(account: account, onRelogin: { showAdd = true })
                }
            }
        }
    }
}

private struct ConsoleAccountCard: View {
    let account: ConsoleAccount
    let onRelogin: () -> Void

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
                        HStack(spacing: 5) {
                            Text(account.platformName)
                                .font(.system(size: 10))
                                .foregroundStyle(ConsoleTheme.secondaryText)
                            if let source = account.sourceLabel {
                                Text(source)
                                    .font(.system(size: 9, weight: .medium))
                                    .foregroundStyle(ConsoleTheme.tertiaryText)
                                    .padding(.horizontal, 4)
                                    .padding(.vertical, 1)
                                    .background(ConsoleTheme.cardSubtle, in: RoundedRectangle(cornerRadius: 3))
                            }
                        }
                    }

                    Spacer(minLength: 6)

                    ConsoleBadge(
                        text: account.displayAuthLabel,
                        tint: ConsoleTheme.tone(account.displayAuthTone)
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

                if !account.authorizations.isEmpty {
                    VStack(alignment: .leading, spacing: 5) {
                        HStack(spacing: 6) {
                            AuthorizationChip(kindLabel: "自有", authorization: account.ownAuthorization)
                            AuthorizationChip(kindLabel: "Magpie", authorization: account.magpieAuthorization)
                            Spacer(minLength: 0)
                        }
                        Text("当前使用 \(account.activeSourceLabel)")
                            .font(.system(size: 10))
                            .foregroundStyle(ConsoleTheme.tertiaryText)
                        if let problem = account.authorizationProblem {
                            Text(problem)
                                .font(.system(size: 10))
                                .foregroundStyle(ConsoleTheme.tone(.warning))
                                .fixedSize(horizontal: false, vertical: true)
                        }
                    }
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
            Button("重新登录", action: onRelogin)
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

/// 一份授权的徽章：谁、什么状态、什么时候到期。
///
/// 两份都在时并排显示，用户能一眼看出"Magpie 有、自有没有"这种情况；
/// 只有一份时另一枚显示"未授权"，而不是干脆不画（否则看起来像只有一种来源）。
private struct AuthorizationChip: View {
    let kindLabel: String
    let authorization: ConsoleAuthorization?

    var body: some View {
        HStack(spacing: 4) {
            Circle()
                .fill(tint)
                .frame(width: 5, height: 5)
            Text(kindLabel)
                .foregroundStyle(ConsoleTheme.secondaryText)
            Text(detail)
                .foregroundStyle(ConsoleTheme.tertiaryText)
        }
        .font(.system(size: 9, weight: .medium))
        .lineLimit(1)
        .padding(.horizontal, 6)
        .padding(.vertical, 3)
        .background(ConsoleTheme.cardSubtle, in: RoundedRectangle(cornerRadius: 5))
    }

    private var tint: Color {
        guard let authorization else { return ConsoleTheme.tertiaryText }
        return ConsoleTheme.tone(authorization.tone)
    }

    private var detail: String {
        guard let authorization else { return "未授权" }
        if authorization.isMigrating { return "迁移中" }
        if let expires = authorization.tokenExpiresAt, expires.count >= 10 {
            return "\(authorization.authLabel) · \(expires.prefix(10))"
        }
        return authorization.authLabel
    }
}
