import AppKit
import SwiftUI

/// 「AI 账号」页（PRD §4）：左侧账号列表 + 右侧账号详情。
///
/// 三条必须守住的界线：
/// 1. 列表行只显示"是谁、哪个平台、一个授权状态"，操作都在右侧详情里；
/// 2. 服务账号（WorkBuddy 账号 / Qoder 服务账号）与**本机 Qoder 配置**分组显示，
///    本机配置不计入服务账号统计；
/// 3. 只渲染真实存在的能力：没有凭据可删、没有接口可调的地方不放假按钮。
struct ConsoleAccountsView: View {
    @ObservedObject var store: ConsoleStore
    @EnvironmentObject private var nav: ConsoleNavigation

    @State private var query = ""
    @State private var activeSheet: ActiveConsoleSheet?
    @State private var verifyNotice: [String: String] = [:]
    @State private var busyAccount: String?

    // 本机 Profile 的操作确认（只读管理 + 显式移除）。
    @State private var renamingProfile: QoderProfile?
    @State private var renameText = ""
    @State private var removingProfile: QoderProfile?
    @State private var showRecoverConfirm = false

    private var trimmedQuery: String {
        query.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
    }

    var body: some View {
        ConsolePageScaffold(scrolls: false) {
            toolbar
        } content: {
            HStack(spacing: 0) {
                listColumn
                    .frame(width: ConsoleTheme.accountListWidth)
                Divider().overlay(ConsoleTheme.border)
                detailColumn
                    .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
            }
            .background(ConsoleTheme.card.opacity(0.35))
            .clipShape(RoundedRectangle(cornerRadius: ConsoleTheme.cardRadius))
            .overlay(
                RoundedRectangle(cornerRadius: ConsoleTheme.cardRadius)
                    .strokeBorder(ConsoleTheme.border, lineWidth: 1)
            )
            .padding(.horizontal, ConsoleTheme.pagePadding)
            .padding(.bottom, 16)
        }
        .task {
            // QA 钩子：`-AICCOpenAddAccount 1` 直接拉起统一添加面板。
            if UserDefaults.standard.bool(forKey: "AICCOpenAddAccount")
                || ProcessInfo.processInfo.environment["AICC_OPEN_ADD_ACCOUNT"] == "1" {
                activeSheet = .addAccount(ConsoleAddRequest())
            }
            if UserDefaults.standard.bool(forKey: "AICCOpenMagpieImport")
                || ProcessInfo.processInfo.environment["AICC_OPEN_MAGPIE_IMPORT"] == "1" {
                activeSheet = .addAccount(ConsoleAddRequest(platform: .workbuddyCN, method: .magpie))
            }
            if store.qoderProfiles.isEmpty && store.qoderEditions.isEmpty {
                await store.scanQoderProfiles(announce: false)
            }
            ensureSelection()
        }
        // 全页只有一个 sheet 路由状态：所有流程从这里进，不并行声明多个 sheet。
        .sheet(item: $activeSheet) { sheet in
            switch sheet {
            case .addAccount(let request):
                ConsoleAddAccountSheet(store: store, request: request) {
                    Task { await store.refreshAccounts() }
                }
            case .qoderProfiles(let edition):
                QoderProfileSheet(store: store, initialEdition: edition)
            case .deleteQoderAccount(let accountId):
                ConsoleDeleteAccountSheet(store: store, accountId: accountId) {
                    nav.accountSelection = nil
                    ensureSelection()
                }
            }
        }
        .alert("重命名本机配置", isPresented: Binding(
            get: { renamingProfile != nil },
            set: { if !$0 { renamingProfile = nil } }
        )) {
            TextField("显示名称", text: $renameText)
            Button("取消", role: .cancel) { renamingProfile = nil }
            Button("保存") {
                if let profile = renamingProfile {
                    let name = renameText
                    Task { await store.qoderRename(profile, to: name) }
                }
                renamingProfile = nil
            }
        } message: {
            Text("只改 AICC 里显示的名称，不会改动 Qoder 客户端里的任何数据。")
        }
        .alert("恢复未完成的切换？", isPresented: $showRecoverConfirm) {
            Button("取消", role: .cancel) {}
            Button("确认恢复") { Task { await store.qoderRecover() } }
        } message: {
            Text("AICC 会先确认对应的 Qoder 客户端已经完全退出；"
                + "无法确认（进程状态未知）时会拒绝恢复，并保持 live 认证原样。")
        }
        .confirmationDialog(
            removingProfile.map { "移除本机配置 \($0.displayName)？" } ?? "移除本机配置",
            isPresented: Binding(
                get: { removingProfile != nil },
                set: { if !$0 { removingProfile = nil } }
            ),
            titleVisibility: .visible
        ) {
            if let profile = removingProfile {
                Button("从 AICC 移除（保留磁盘数据）") {
                    Task { await store.qoderRemove(profile, purge: false) }
                    removingProfile = nil
                }
                Button("永久删除快照与用户数据目录", role: .destructive) {
                    Task { await store.qoderRemove(profile, purge: true) }
                    removingProfile = nil
                }
                Button("取消", role: .cancel) { removingProfile = nil }
            }
        } message: {
            if let profile = removingProfile {
                Text(
                    "「从 AICC 移除」只删登记，Qoder 客户端里的登录状态与本地数据都保留。\n"
                    + "「永久删除」会删除 AICC 保存的账号快照 \(profile.snapshotDir)，无法撤销"
                    + "（客户端数据目录不会被动）。"
                )
            }
        }
    }

    // MARK: - 工具栏

    private var toolbar: some View {
        HStack(spacing: 8) {
            ConsoleSearchField(placeholder: "搜索昵称、UID 或邮箱", text: $query)
                .frame(minWidth: 140, maxWidth: 280)

            ConsoleFilterMenu(
                title: "平台筛选",
                options: ConsolePlatformFilter.accountFilters,
                label: { $0.title },
                selection: $nav.accountsFilter
            )

            Text("\(visibleCount) 个")
                .font(ConsoleType.small)
                .foregroundStyle(ConsoleTheme.secondaryText)
                .lineLimit(1)

            Spacer(minLength: 6)

            Button {
                activeSheet = .qoderProfiles(.global)
            } label: {
                Label("本机配置", systemImage: "internaldrive")
            }
            .buttonStyle(ConsoleSecondaryButtonStyle())
            .help("管理本机 Qoder 客户端的认证快照（只读，不开放账号切换）")

            Button {
                activeSheet = .addAccount(ConsoleAddRequest())
            } label: {
                Label("添加账号", systemImage: "plus")
            }
            .buttonStyle(ConsolePrimaryButtonStyle())
            .help("选择平台与授权方式，添加 WorkBuddy 或 Qoder 账号")
        }
    }

    // MARK: - 左栏列表

    private struct AccountListEntry: Identifiable {
        let id: String
        let selection: ConsoleAccountSelection
        let title: String
        let subtitle: String
        let symbol: String
        let tone: ConsoleTheme.Tone
        let statusText: String
    }

    private struct AccountListSection: Identifiable {
        let id: String
        let title: String
        let entries: [AccountListEntry]
    }

    private var sections: [AccountListSection] {
        var result: [AccountListSection] = []

        func workbuddyEntries(international: Bool) -> [AccountListEntry] {
            store.accounts
                .filter { $0.isWorkbuddy(international: international) }
                .filter { trimmedQuery.isEmpty || $0.searchText.contains(trimmedQuery) }
                .map { account in
                    AccountListEntry(
                        id: "wb-\(account.id)",
                        selection: .workbuddy(account.id),
                        title: account.nickname,
                        subtitle: "UID \(ConsoleFormat.maskedUID(account.uid))",
                        symbol: account.symbolName,
                        tone: ConsoleTheme.consoleTone(account.displayAuthTone),
                        statusText: account.displayAuthLabel
                    )
                }
        }

        func qoderServiceEntries(edition: QoderEdition) -> [AccountListEntry] {
            store.qoderServiceAccounts
                .filter { $0.edition == edition.rawValue }
                .filter { trimmedQuery.isEmpty || $0.searchText.contains(trimmedQuery) }
                .map { account in
                    AccountListEntry(
                        id: "qs-\(account.id)",
                        selection: .qoderService(account.id),
                        title: account.displayName,
                        subtitle: account.email.map(ConsoleFormat.maskedEmail)
                            ?? "UID \(account.maskedUid)",
                        symbol: edition.symbolName,
                        tone: tone(for: account.statusTone),
                        statusText: account.statusLabel
                    )
                }
        }

        func profileEntries(edition: QoderEdition) -> [AccountListEntry] {
            store.qoderProfiles
                .filter { $0.edition == edition.rawValue }
                .filter { trimmedQuery.isEmpty || $0.searchText.contains(trimmedQuery) }
                .map { profile in
                    AccountListEntry(
                        id: "qp-\(profile.id)",
                        selection: .qoderProfile(profile.id),
                        title: profile.displayName,
                        subtitle: profile.identityText ?? profile.identityLabel,
                        symbol: profile.symbolName,
                        tone: ConsoleTheme.consoleTone(profile.authTone),
                        statusText: profile.authLabel
                    )
                }
        }

        if nav.accountsFilter.showsWorkbuddyCN {
            let entries = workbuddyEntries(international: false)
            if !entries.isEmpty {
                result.append(AccountListSection(id: "wb-cn", title: "WorkBuddy CN", entries: entries))
            }
        }
        if nav.accountsFilter.showsWorkbuddyAI {
            let entries = workbuddyEntries(international: true)
            if !entries.isEmpty {
                result.append(AccountListSection(id: "wb-ai", title: "WorkBuddy AI", entries: entries))
            }
        }
        if nav.accountsFilter.showsQoder {
            for edition in QoderEdition.allCases {
                guard nav.accountsFilter.edition == nil || nav.accountsFilter.edition == edition else { continue }
                let serviceEntries = qoderServiceEntries(edition: edition)
                let profiles = profileEntries(edition: edition)
                if !serviceEntries.isEmpty {
                    result.append(AccountListSection(
                        id: "qs-\(edition.rawValue)",
                        title: edition.displayName,
                        entries: serviceEntries
                    ))
                }
                if !profiles.isEmpty {
                    result.append(AccountListSection(
                        id: "qp-\(edition.rawValue)",
                        title: "\(edition.displayName) · 本机配置",
                        entries: profiles
                    ))
                }
            }
        }
        return result
    }

    private var visibleCount: Int {
        sections.reduce(0) { $0 + $1.entries.count }
    }

    private func tone(for status: QoderServiceStatusTone) -> ConsoleTheme.Tone {
        switch status {
        case .ok: return .ok
        case .warn: return .warning
        case .muted: return .neutral
        }
    }

    private var listColumn: some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 2, pinnedViews: []) {
                if !store.qoderPendingSwitches.isEmpty {
                    pendingSwitchNotice
                        .padding(.horizontal, 8)
                        .padding(.top, 8)
                }
                let sections = sections
                if sections.isEmpty {
                    // 主操作只有一个：工具栏的「添加账号」。空状态不重复放按钮。
                    ConsoleEmptyState(
                        symbol: sectionsPlaceholderSymbol,
                        title: sectionsPlaceholderTitle,
                        message: sectionsPlaceholderMessage
                    )
                } else {
                    ForEach(sections) { section in
                        Text(section.title)
                            .font(ConsoleType.caption)
                            .foregroundStyle(ConsoleTheme.tertiaryText)
                            .padding(.horizontal, 10)
                            .padding(.top, 10)
                            .padding(.bottom, 2)
                        ForEach(section.entries) { entry in
                            listRow(entry)
                        }
                    }
                }
            }
            .padding(.bottom, 10)
        }
        .frame(maxHeight: .infinity)
    }

    private var sectionsPlaceholderSymbol: String {
        store.accounts.isEmpty && store.qoderServiceAccounts.isEmpty && store.qoderProfiles.isEmpty
            ? "person.crop.rectangle.stack" : "line.3.horizontal.decrease.circle"
    }

    private var sectionsPlaceholderTitle: String {
        store.accounts.isEmpty && store.qoderServiceAccounts.isEmpty && store.qoderProfiles.isEmpty
            ? "还没有账号" : "没有匹配的账号"
    }

    private var sectionsPlaceholderMessage: String {
        store.accounts.isEmpty && store.qoderServiceAccounts.isEmpty && store.qoderProfiles.isEmpty
            ? "可以添加 WorkBuddy 或 Qoder 账号，也可以先扫描本机已有的 Qoder 配置。"
            : "换个关键词，或把平台筛选切回「全部」。"
    }

    private func listRow(_ entry: AccountListEntry) -> some View {
        let selected = nav.accountSelection == entry.selection
        return Button {
            nav.accountSelection = entry.selection
            store.clearQoderDeleteFeedback()
        } label: {
            HStack(spacing: 8) {
                Image(systemName: entry.symbol)
                    .font(.system(size: 11))
                    .foregroundStyle(selected ? ConsoleTheme.accent : ConsoleTheme.secondaryText)
                    .frame(width: 16)
                VStack(alignment: .leading, spacing: 1) {
                    Text(entry.title)
                        .font(ConsoleType.rowTitle)
                        .foregroundStyle(ConsoleTheme.text)
                        .lineLimit(1)
                    Text(entry.subtitle)
                        .font(ConsoleType.caption)
                        .foregroundStyle(ConsoleTheme.tertiaryText)
                        .lineLimit(1)
                        .truncationMode(.middle)
                }
                Spacer(minLength: 4)
                Circle()
                    .fill(ConsoleTheme.color(entry.tone))
                    .frame(width: 6, height: 6)
            }
            .padding(.horizontal, 8)
            .frame(height: 40)
            .background(
                selected ? ConsoleTheme.selectionFill : Color.clear,
                in: RoundedRectangle(cornerRadius: 7)
            )
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .help("\(entry.title) · \(entry.statusText)")
        .padding(.horizontal, 4)
    }

    private var pendingSwitchNotice: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text("检测到 \(store.qoderPendingSwitches.count) 条未完成的切换记录")
                .font(ConsoleType.caption)
                .foregroundStyle(ConsoleTheme.warning)
                .fixedSize(horizontal: false, vertical: true)
            Button("手动恢复…") { showRecoverConfirm = true }
                .buttonStyle(ConsoleSecondaryButtonStyle())
                .disabled(store.qoderActing != nil)
        }
        .padding(8)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(ConsoleTheme.warningSoft, in: RoundedRectangle(cornerRadius: 8))
    }

    // MARK: - 右栏详情

    @ViewBuilder
    private var detailColumn: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: ConsoleTheme.sectionSpacing) {
                if let error = store.accountsError {
                    ConsoleInlineNotice(kind: .error, message: error, actionTitle: "重试") {
                        Task { await store.refreshAccounts() }
                    }
                }
                if let error = store.qoderError {
                    ConsoleInlineNotice(kind: .error, message: error, actionTitle: "重试") {
                        Task { await store.refreshQoder() }
                    }
                }
                if let notice = store.qoderDeleteNotice {
                    ConsoleInlineNotice(kind: .info, message: notice)
                }
                if let error = store.qoderDeleteError {
                    ConsoleInlineNotice(kind: .error, message: error, actionTitle: "重试") {
                        if let target = currentQoderService {
                            activeSheet = .deleteQoderAccount(target.id)
                        }
                    }
                }
                if let notice = store.qoderNotice {
                    ConsoleInlineNotice(kind: .info, message: notice)
                }
                detailContent
            }
            .padding(14)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
    }

    @ViewBuilder
    private var detailContent: some View {
        switch resolvedSelection {
        case .none:
            ConsoleEmptyState(
                symbol: "sidebar.left",
                title: "未选择账号",
                message: visibleCount == 0
                    ? "用右上角「添加账号」新增一个 WorkBuddy 或 Qoder 账号。"
                    : "在左侧选择一个账号查看授权来源与可用能力。"
            )
        case .some(.workbuddy(let id)):
            if let account = store.accounts.first(where: { $0.id == id }) {
                workbuddyDetail(account)
            } else {
                missingAccount
            }
        case .some(.qoderService(let id)):
            if let account = store.qoderServiceAccounts.first(where: { $0.id == id }) {
                qoderServiceDetail(account)
            } else {
                missingAccount
            }
        case .some(.qoderProfile(let id)):
            if let profile = store.qoderProfiles.first(where: { $0.id == id }) {
                qoderProfileDetail(profile)
            } else {
                missingAccount
            }
        }
    }

    private var missingAccount: some View {
        ConsoleEmptyState(
            symbol: "questionmark.folder",
            title: "账号已不在列表里",
            message: "它可能刚被删除或刷新后不再返回。左侧重新选一个账号。"
        )
    }

    private var currentQoderService: QoderServiceAccount? {
        if case .qoderService(let id) = resolvedSelection {
            return store.qoderServiceAccounts.first { $0.id == id }
        }
        return nil
    }

    /// 选中项失效时回落到列表第一个，避免右侧空白。
    private var resolvedSelection: ConsoleAccountSelection? {
        if let selection = nav.accountSelection, isAvailable(selection) { return selection }
        return sections.first?.entries.first?.selection
    }

    private func isAvailable(_ selection: ConsoleAccountSelection) -> Bool {
        switch selection {
        case .workbuddy(let id): return store.accounts.contains { $0.id == id }
        case .qoderService(let id): return store.qoderServiceAccounts.contains { $0.id == id }
        case .qoderProfile(let id): return store.qoderProfiles.contains { $0.id == id }
        }
    }

    private func ensureSelection() {
        if nav.accountSelection == nil {
            nav.accountSelection = sections.first?.entries.first?.selection
        }
    }

    // MARK: WorkBuddy 详情

    private func workbuddyDetail(_ account: ConsoleAccount) -> some View {
        let international = account.isWorkbuddyAI
        return VStack(alignment: .leading, spacing: ConsoleTheme.sectionSpacing) {
            detailHeader(
                symbol: account.symbolName,
                title: account.nickname,
                subtitle: account.platformName + (international ? " · 国际版" : " · 国内版"),
                statusText: account.displayAuthLabel,
                tone: ConsoleTheme.consoleTone(account.displayAuthTone)
            )

            ConsoleDetailGroup(title: "基本资料") {
                VStack(alignment: .leading, spacing: 8) {
                    ConsoleInfoRow(label: "平台", value: account.platformName)
                    ConsoleInfoRow(label: "账号昵称", value: account.nickname)
                    ConsoleInfoRow(label: "UID", value: ConsoleFormat.maskedUID(account.uid), mono: true)
                    ConsoleInfoRow(label: "邮箱", value: "未提供（该平台接口不返回邮箱）")
                    ConsoleInfoRow(
                        label: "最近更新",
                        value: ConsoleFormat.shortTimestamp(account.updatedAt)
                    )
                }
            }

            ConsoleDetailGroup(title: "授权来源", detail: "共 \(account.authorizations.count) 份") {
                VStack(alignment: .leading, spacing: 10) {
                    if account.authorizations.isEmpty {
                        Text("这个账号还没有登记任何授权来源，请重新登录一次。")
                            .font(ConsoleType.small)
                            .foregroundStyle(ConsoleTheme.secondaryText)
                    } else {
                        ForEach(account.authorizations) { authorization in
                            authorizationRow(
                                title: authorization.isOwn ? "浏览器登录（自有）" : "Magpie 关联",
                                authorization: authorization,
                                note: authorization.isOwn
                                    ? "AICC 自己持有的令牌"
                                    : "令牌归 Magpie 刷新，AICC 只借用，不能单独验证"
                            )
                            Divider().overlay(ConsoleTheme.border)
                        }
                    }
                    HStack(spacing: 8) {
                        Button("重新授权") {
                            activeSheet = .addAccount(ConsoleAddRequest(
                                platform: international ? .workbuddyAI : .workbuddyCN,
                                method: .browser
                            ))
                        }
                        .buttonStyle(ConsoleSecondaryButtonStyle())

                        Button("验证授权") { Task { await verify(account) } }
                            .buttonStyle(ConsoleSecondaryButtonStyle())
                            .disabled(busyAccount == account.id)

                        if busyAccount == account.id {
                            ProgressView().controlSize(.mini)
                        }
                        Spacer(minLength: 0)
                    }
                    if let notice = verifyNotice[account.id] {
                        Text(notice)
                            .font(ConsoleType.caption)
                            .foregroundStyle(ConsoleTheme.secondaryText)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                    Text("当前使用：\(account.activeSourceLabel)"
                        + (account.tokenExpiresAt.map { " · 到期 \(ConsoleFormat.shortTimestamp($0))" } ?? ""))
                        .font(ConsoleType.caption)
                        .foregroundStyle(ConsoleTheme.tertiaryText)
                }
            }

            ConsoleDetailGroup(title: "可用能力", detail: "只显示真实接入的能力") {
                VStack(alignment: .leading, spacing: 8) {
                    capabilityRow("每日签到", enabled: account.supportsCNTasks,
                                  note: account.supportsCNTasks ? "已接入" : "国际版没有该接口")
                    capabilityRow("成长中心", enabled: account.supportsCNTasks,
                                  note: account.supportsCNTasks ? "已接入" : "国际版没有该接口")
                    capabilityRow("Buddy 旅行", enabled: account.supportsCNTasks,
                                  note: account.supportsCNTasks ? "已接入" : "国际版没有该接口")
                    capabilityRow("官方额度", enabled: true, note: "账号额度看板")
                }
            }

            ConsoleDetailGroup(title: "更多操作") {
                HStack(spacing: 8) {
                    Button("刷新账号信息") { Task { await verify(account) } }
                        .buttonStyle(ConsoleSecondaryButtonStyle())
                        .disabled(busyAccount == account.id)
                    Button("重新登录") {
                        activeSheet = .addAccount(ConsoleAddRequest(
                            platform: international ? .workbuddyAI : .workbuddyCN,
                            method: .browser
                        ))
                    }
                    .buttonStyle(ConsoleSecondaryButtonStyle())
                    Button("复制 UID") {
                        NSPasteboard.general.clearContents()
                        NSPasteboard.general.setString(account.uid, forType: .string)
                    }
                    .buttonStyle(ConsoleSecondaryButtonStyle())
                    Spacer(minLength: 0)
                    Button("查看额度") {
                        nav.page = .quota
                        nav.quotaSource = .accounts
                    }
                    .buttonStyle(ConsoleSecondaryButtonStyle())
                }
            }
        }
        .task(id: account.id) { verifyNotice[account.id] = nil }
    }

    // MARK: Qoder 服务账号详情

    private func qoderServiceDetail(_ account: QoderServiceAccount) -> some View {
        let edition = QoderEdition(rawValue: account.edition) ?? .global
        let deleting = store.qoderDeleting == account.id
        return VStack(alignment: .leading, spacing: ConsoleTheme.sectionSpacing) {
            detailHeader(
                symbol: edition.symbolName,
                title: account.displayName,
                subtitle: account.editionName,
                statusText: account.statusLabel,
                tone: tone(for: account.statusTone)
            )

            ConsoleDetailGroup(title: "基本资料") {
                VStack(alignment: .leading, spacing: 8) {
                    ConsoleInfoRow(label: "平台", value: account.editionName)
                    ConsoleInfoRow(label: "账号昵称", value: account.displayName)
                    ConsoleInfoRow(label: "UID", value: ConsoleFormat.maskedUID(account.uid), mono: true)
                    ConsoleInfoRow(
                        label: "邮箱",
                        value: account.email.map(ConsoleFormat.maskedEmail) ?? "未提供"
                    )
                    ConsoleInfoRow(label: "最近更新", value: lastQoderSyncText)
                }
            }

            ConsoleDetailGroup(
                title: "授权来源",
                detail: account.authorizations.isEmpty ? "暂无" : "共 \(account.authorizations.count) 份"
            ) {
                VStack(alignment: .leading, spacing: 10) {
                    if account.authorizations.isEmpty {
                        Text("还没有授权来源。用浏览器授权或 PAT 添加一份。")
                            .font(ConsoleType.small)
                            .foregroundStyle(ConsoleTheme.secondaryText)
                    } else {
                        ForEach(Array(account.authorizations.enumerated()), id: \.offset) { _, authorization in
                            qoderAuthorizationRow(authorization, account: account)
                            Divider().overlay(ConsoleTheme.border)
                        }
                    }
                    HStack(spacing: 8) {
                        Button("浏览器授权") {
                            activeSheet = .addAccount(ConsoleAddRequest(platform: platformKind(for: edition), method: .browser))
                        }
                        .buttonStyle(ConsoleSecondaryButtonStyle())
                        Button("添加 PAT") {
                            activeSheet = .addAccount(ConsoleAddRequest(platform: platformKind(for: edition), method: .pat))
                        }
                        .buttonStyle(ConsoleSecondaryButtonStyle())
                        Spacer(minLength: 0)
                    }
                    Text("Magpie 关联只说明该账号在 Magpie 里登录过，AICC 手里没有它的凭据，因此不算已验证。")
                        .font(ConsoleType.caption)
                        .foregroundStyle(ConsoleTheme.tertiaryText)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }

            ConsoleDetailGroup(title: "可用能力", detail: "按版本实际支持情况") {
                VStack(alignment: .leading, spacing: 8) {
                    capabilityRow(
                        "每日 Credits 活动",
                        enabled: account.edition == "qoder_cn" && !account.usableSources.isEmpty,
                        note: account.edition == "qoder_cn"
                            ? (account.usableSources.isEmpty ? "需要有效授权" : "已接入（CN）")
                            : "该版本未验证，不会发请求"
                    )
                    capabilityRow("官方额度余额", enabled: false, note: "服务端没有余额接口")
                    capabilityRow("本机 Profile 快照", enabled: false, note: "见「本机配置」分类")
                }
            }

            ConsoleDetailGroup(title: "更多操作") {
                VStack(alignment: .leading, spacing: 8) {
                    HStack(spacing: 8) {
                        Button("刷新账号信息") { Task { await store.refreshQoder() } }
                            .buttonStyle(ConsoleSecondaryButtonStyle())
                        Button("到签到任务") {
                            nav.openCheckin(platform: .qoder)
                        }
                        .buttonStyle(ConsoleSecondaryButtonStyle())
                        Spacer(minLength: 0)
                        Button(deleting ? "移除中…" : "从 AICC 移除账号") {
                            activeSheet = .deleteQoderAccount(account.id)
                        }
                        .buttonStyle(ConsoleSecondaryButtonStyle())
                        .disabled(deleting || store.qoderDeleting != nil)
                        .help("删除 AICC 保存的凭据与记录，不动 Magpie 与本机 Profile")
                    }
                    Text("移除只影响 AICC：Magpie 配置与 Qoder 客户端本机 Profile 都会保留，"
                        + "也不会撤销服务端 PAT。")
                        .font(ConsoleType.caption)
                        .foregroundStyle(ConsoleTheme.tertiaryText)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
        }
    }

    private var lastQoderSyncText: String {
        store.qoderSyncedAt.map(ConsoleFormat.shortDateTime) ?? "尚未同步"
    }

    private func platformKind(for edition: QoderEdition) -> ConsolePlatformKind {
        edition == .cn ? .qoderCN : .qoderGlobal
    }

    private func qoderAuthorizationRow(
        _ authorization: QoderAuthorization,
        account: QoderServiceAccount
    ) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Text(authorization.sourceLabel)
                    .font(ConsoleType.rowTitle)
                    .foregroundStyle(ConsoleTheme.text)
                ConsoleBadge(
                    text: authorization.statusLabel,
                    tint: authorization.isUsable
                        ? ConsoleTheme.success
                        : (authorization.authStatus == "expired" ? ConsoleTheme.warning : ConsoleTheme.tertiaryText)
                )
                if !authorization.hasSecret {
                    Text("无本地凭据")
                        .font(ConsoleType.caption)
                        .foregroundStyle(ConsoleTheme.tertiaryText)
                }
                Spacer(minLength: 6)
                if let expires = authorization.expiresAt {
                    Text("到期 \(ConsoleFormat.shortTimestamp(expires))")
                        .font(ConsoleType.caption)
                        .foregroundStyle(ConsoleTheme.tertiaryText)
                }
            }
            if authorization.source == "own_pat" {
                HStack(spacing: 8) {
                    Button("检测 PAT") {
                        Task { await checkPat(account) }
                    }
                    .buttonStyle(ConsoleSecondaryButtonStyle())
                    .disabled(busyAccount == account.id)
                    Button("移除这份 PAT") {
                        Task { await removePat(account) }
                    }
                    .buttonStyle(ConsoleSecondaryButtonStyle())
                    .disabled(busyAccount == account.id)
                    if busyAccount == account.id {
                        ProgressView().controlSize(.mini)
                    }
                    Spacer(minLength: 0)
                }
                Text("检测只确认身份仍有效，不代表 Credits 权限也还在。")
                    .font(ConsoleType.caption)
                    .foregroundStyle(ConsoleTheme.tertiaryText)
            }
        }
    }

    // MARK: 本机 Qoder 配置详情

    private func qoderProfileDetail(_ profile: QoderProfile) -> some View {
        let edition = QoderEdition(rawValue: profile.edition) ?? .global
        let info = store.qoderEditions.first { $0.edition == profile.edition }
        let busy = store.qoderActing == profile.id
        return VStack(alignment: .leading, spacing: ConsoleTheme.sectionSpacing) {
            detailHeader(
                symbol: profile.symbolName,
                title: profile.displayName,
                subtitle: "\(edition.displayName) · 本机配置",
                statusText: profile.authLabel,
                tone: ConsoleTheme.consoleTone(profile.authTone)
            )

            ConsoleDetailGroup(title: "基本资料") {
                VStack(alignment: .leading, spacing: 8) {
                    ConsoleInfoRow(label: "版本", value: edition.displayName)
                    ConsoleInfoRow(label: "托管方式", value: profile.managed ? "AICC 托管" : "仅登记引用")
                    ConsoleInfoRow(label: "身份", value: profile.identityText ?? "未识别")
                    ConsoleInfoRow(label: "Token 指纹", value: profile.fingerprintShort, mono: true)
                    ConsoleInfoRow(label: "到期", value: profile.expiresAt.map { String($0.prefix(10)) } ?? "—")
                    ConsoleInfoRow(label: "最后打开", value: ConsoleFormat.shortTimestamp(profile.lastOpenedAt))
                }
            }

            ConsoleDetailGroup(title: "快照与运行状态") {
                VStack(alignment: .leading, spacing: 8) {
                    HStack(spacing: 8) {
                        ConsoleBadge(text: profile.runtimeLabel, tint: ConsoleTheme.color(ConsoleTheme.consoleTone(profile.runtimeTone)))
                        ConsoleBadge(
                            text: profile.snapshotReady ? "快照完整" : "无完整快照",
                            tint: profile.snapshotReady ? ConsoleTheme.success : ConsoleTheme.warning
                        )
                        if profile.active {
                            ConsoleBadge(text: "客户端当前登录", tint: ConsoleTheme.accent)
                        }
                        Spacer(minLength: 0)
                    }
                    ConsoleInfoRow(label: "快照目录", value: profile.snapshotDir, mono: true)
                    ConsoleInfoRow(label: "客户端目录", value: profile.liveDir, mono: true)
                    if let info, !info.installed {
                        ConsoleInlineNotice(
                            kind: .warning,
                            message: "本机没有检测到 \(edition.displayName) 客户端（Bundle \(info.bundleId)）。"
                        )
                    }
                    if !profile.switchEnabled, let reason = profile.switchBlockedReason {
                        ConsoleInlineNotice(kind: .info, message: "账号切换未开放：\(reason)")
                    }
                    if profile.runtime.isUnknown {
                        ConsoleInlineNotice(
                            kind: .warning,
                            message: "有进程存活但无法确认归属，AICC 不会对它发信号。"
                        )
                    } else if profile.snapshotIncomplete {
                        ConsoleInlineNotice(
                            kind: .warning,
                            message: "上次保存快照没有完成，快照可能只有半套文件；再次保存会先回到上一份完整快照。"
                        )
                    }
                }
            }

            ConsoleDetailGroup(title: "更多操作", detail: "只读管理") {
                HStack(spacing: 8) {
                    if profile.runtime.isRunning {
                        Button("关闭客户端") { Task { await store.qoderClose(profile) } }
                            .buttonStyle(ConsoleSecondaryButtonStyle())
                    } else {
                        Button("打开客户端") { Task { await store.qoderOpen(profile) } }
                            .buttonStyle(ConsoleSecondaryButtonStyle())
                    }
                    Button("保存当前登录") { Task { await store.qoderCapture(profile) } }
                        .buttonStyle(ConsoleSecondaryButtonStyle())
                    Button("检查授权") { Task { await store.qoderInspect(profile) } }
                        .buttonStyle(ConsoleSecondaryButtonStyle())
                    Button("重命名") {
                        renameText = profile.displayName
                        renamingProfile = profile
                    }
                    .buttonStyle(ConsoleSecondaryButtonStyle())
                    Spacer(minLength: 0)
                    Button("移除…") { removingProfile = profile }
                        .buttonStyle(ConsoleSecondaryButtonStyle())
                        .disabled(profile.runtime.isRunning)
                        .help(profile.runtime.isRunning ? "运行中的配置不能删除，请先关闭客户端" : "从 AICC 移除这个本机配置")
                }
                .disabled(busy)
                .opacity(busy ? 0.55 : 1)
            }
        }
    }

    // MARK: 共享小件

    private func detailHeader(
        symbol: String,
        title: String,
        subtitle: String,
        statusText: String,
        tone: ConsoleTheme.Tone
    ) -> some View {
        HStack(spacing: 10) {
            Image(systemName: symbol)
                .font(.system(size: 13, weight: .semibold))
                .foregroundStyle(ConsoleTheme.accent)
                .frame(width: 30, height: 30)
                .background(ConsoleTheme.accentSoft, in: RoundedRectangle(cornerRadius: 8))
            VStack(alignment: .leading, spacing: 1) {
                Text(title)
                    .font(.system(size: 15, weight: .semibold))
                    .foregroundStyle(ConsoleTheme.text)
                    .lineLimit(2)
                    .fixedSize(horizontal: false, vertical: true)
                Text(subtitle)
                    .font(ConsoleType.caption)
                    .foregroundStyle(ConsoleTheme.secondaryText)
                    .lineLimit(1)
            }
            Spacer(minLength: 8)
            ConsoleBadge(text: statusText, tint: ConsoleTheme.color(tone))
        }
    }

    private func authorizationRow(
        title: String,
        authorization: ConsoleAuthorization,
        note: String
    ) -> some View {
        VStack(alignment: .leading, spacing: 3) {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Text(title)
                    .font(ConsoleType.rowTitle)
                    .foregroundStyle(ConsoleTheme.text)
                ConsoleBadge(
                    // Magpie 来源的令牌归 Magpie，AICC 只借用：不能显示成"已授权"。
                    text: authorization.isOwn
                        ? authorization.authLabel
                        : (authorization.authLabel == "已授权" ? "已关联" : authorization.authLabel),
                    tint: ConsoleTheme.tone(authorization.tone)
                )
                if authorization.isMigrating {
                    ConsoleBadge(text: "迁移中", tint: ConsoleTheme.warning)
                }
                Spacer(minLength: 6)
                if let expires = authorization.tokenExpiresAt {
                    Text("到期 \(ConsoleFormat.shortTimestamp(expires))")
                        .font(ConsoleType.caption)
                        .foregroundStyle(ConsoleTheme.tertiaryText)
                }
            }
            Text(note)
                .font(ConsoleType.caption)
                .foregroundStyle(ConsoleTheme.tertiaryText)
                .fixedSize(horizontal: false, vertical: true)
            if let error = authorization.lastError, !error.isEmpty {
                Text(error)
                    .font(ConsoleType.caption)
                    .foregroundStyle(ConsoleTheme.warning)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }

    private func capabilityRow(_ name: String, enabled: Bool, note: String) -> some View {
        HStack(spacing: 8) {
            Image(systemName: enabled ? "checkmark.circle.fill" : "minus.circle")
                .font(.system(size: 11))
                .foregroundStyle(enabled ? ConsoleTheme.success : ConsoleTheme.tertiaryText)
            Text(name)
                .font(ConsoleType.body)
                .foregroundStyle(ConsoleTheme.text)
            Spacer(minLength: 6)
            Text(note)
                .font(ConsoleType.caption)
                .foregroundStyle(ConsoleTheme.tertiaryText)
                .lineLimit(1)
        }
    }

    // MARK: - 行为

    private func verify(_ account: ConsoleAccount) async {
        busyAccount = account.id
        defer { busyAccount = nil }
        verifyNotice[account.id] = "检查中…"
        verifyNotice[account.id] = await store.verifyWorkbuddyAccount(account)
    }

    private func checkPat(_ account: QoderServiceAccount) async {
        busyAccount = account.id
        defer { busyAccount = nil }
        do {
            switch try await store.worker.checkQoderPat(accountId: account.id) {
            case .success(let name):
                store.showQoderNotice("PAT 身份有效：\(name)（Credits 权限另需检测）")
            case .failure(let error):
                store.showQoderNotice("\(error.message)\(error.hint.isEmpty ? "" : " " + error.hint)")
            }
        } catch {
            store.showQoderNotice((error as? LocalizedError)?.errorDescription ?? error.localizedDescription)
        }
        await store.refreshQoder(silent: true)
    }

    private func removePat(_ account: QoderServiceAccount) async {
        busyAccount = account.id
        defer { busyAccount = nil }
        do {
            _ = try await store.worker.removeQoderPat(accountId: account.id)
            store.showQoderNotice("已移除 PAT，其余来源保持不变")
        } catch {
            store.showQoderNotice((error as? LocalizedError)?.errorDescription ?? error.localizedDescription)
        }
        await store.refreshQoder(silent: true)
    }
}

// MARK: - 本机 Profile 添加 / 导入

struct QoderProfileSheet: View {
    @ObservedObject var store: ConsoleStore
    let initialEdition: QoderEdition

    @Environment(\.dismiss) private var dismiss
    @State private var edition: QoderEdition = .global
    @State private var mode: Mode = .create
    @State private var name = ""
    @State private var path = ""

    private enum Mode: String, CaseIterable, Identifiable {
        case create
        case importExisting

        var id: String { rawValue }
        var title: String {
            switch self {
            case .create: return "新建配置"
            case .importExisting: return "导入已有目录"
            }
        }
    }

    private var info: QoderEditionInfo? {
        store.qoderEditions.first { $0.edition == edition.rawValue }
    }

    private var canSubmit: Bool {
        switch mode {
        case .create: return true
        case .importExisting:
            return path.hasPrefix("/") && !path.trimmingCharacters(in: .whitespaces).isEmpty
        }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            HStack(spacing: 9) {
                Image(systemName: "internaldrive")
                    .font(.system(size: 13, weight: .semibold))
                    .foregroundStyle(ConsoleTheme.accent)
                    .frame(width: 28, height: 28)
                    .background(ConsoleTheme.accentSoft, in: RoundedRectangle(cornerRadius: 8))
                VStack(alignment: .leading, spacing: 1) {
                    Text("本机 Qoder 配置")
                        .font(ConsoleType.sectionTitle)
                        .foregroundStyle(ConsoleTheme.text)
                    Text("只读管理：识别、扫描、导入、保存快照；不开放账号切换")
                        .font(ConsoleType.caption)
                        .foregroundStyle(ConsoleTheme.secondaryText)
                }
                Spacer(minLength: 0)
            }

            Picker("版本", selection: $edition) {
                ForEach(QoderEdition.allCases) { item in
                    Text(item.displayName).tag(item)
                }
            }
            .pickerStyle(.segmented)
            .labelsHidden()
            .tint(ConsoleTheme.accent)

            if let info {
                HStack(spacing: 6) {
                    Image(systemName: info.installed ? "checkmark.circle" : "exclamationmark.triangle")
                        .font(ConsoleType.caption)
                    Text("\(info.bundleId) · \(info.installSummary)")
                        .font(ConsoleType.caption)
                        .lineLimit(1)
                        .truncationMode(.middle)
                }
                .foregroundStyle(info.installed ? ConsoleTheme.tertiaryText : ConsoleTheme.warning)
            }

            Picker("方式", selection: $mode) {
                ForEach(Mode.allCases) { item in
                    Text(item.title).tag(item)
                }
            }
            .pickerStyle(.segmented)
            .labelsHidden()
            .tint(ConsoleTheme.accent)

            VStack(alignment: .leading, spacing: 5) {
                Text("显示名称")
                    .font(ConsoleType.caption)
                    .foregroundStyle(ConsoleTheme.tertiaryText)
                TextField(edition.defaultProfileName, text: $name)
                    .textFieldStyle(.roundedBorder)
                    .font(ConsoleType.body)
            }

            if mode == .importExisting {
                VStack(alignment: .leading, spacing: 5) {
                    Text("已有账号快照目录（里面要有 auth.v1.dat）")
                        .font(ConsoleType.caption)
                        .foregroundStyle(ConsoleTheme.tertiaryText)
                    HStack(spacing: 8) {
                        TextField("选择包含 auth.v1.dat 的目录", text: $path)
                            .textFieldStyle(.roundedBorder)
                            .font(ConsoleType.mono)
                        Button("选择…") { pickDirectory() }
                            .buttonStyle(ConsoleSecondaryButtonStyle())
                    }
                    Text("只登记引用：不移动、不覆盖任何文件，也不要求重新登录。")
                        .font(ConsoleType.caption)
                        .foregroundStyle(ConsoleTheme.tertiaryText)
                }
            } else {
                Text("会创建一个空的账号快照。创建后请手动打开 \(edition.displayName) 客户端登录，"
                    + "登录完成再回 AICC 点「保存当前登录」。AICC 不接触账号密码。")
                    .font(ConsoleType.caption)
                    .foregroundStyle(ConsoleTheme.tertiaryText)
                    .fixedSize(horizontal: false, vertical: true)
            }

            Divider().overlay(ConsoleTheme.border)

            HStack(spacing: 8) {
                Button("取消") { dismiss() }
                    .buttonStyle(ConsoleSecondaryButtonStyle())
                Spacer()
                Button(mode == .create ? "创建" : "导入") {
                    let displayName = name.trimmingCharacters(in: .whitespacesAndNewlines)
                    let finalName = displayName.isEmpty ? edition.defaultProfileName : displayName
                    let importPath = path.trimmingCharacters(in: .whitespacesAndNewlines)
                    let importMode = mode == .importExisting
                    let targetEdition = edition
                    dismiss()
                    Task {
                        if importMode {
                            await store.importQoderProfile(
                                edition: targetEdition,
                                snapshotDir: importPath,
                                displayName: finalName
                            )
                        } else {
                            await store.addQoderProfile(
                                edition: targetEdition,
                                displayName: finalName
                            )
                        }
                    }
                }
                .buttonStyle(ConsolePrimaryButtonStyle())
                .disabled(!canSubmit)
            }
        }
        .padding(18)
        .frame(width: 460)
        .background(ConsoleTheme.canvas)
        .onAppear {
            edition = initialEdition
            name = initialEdition.defaultProfileName
        }
    }

    private func pickDirectory() {
        let panel = NSOpenPanel()
        panel.canChooseDirectories = true
        panel.canChooseFiles = false
        panel.allowsMultipleSelection = false
        panel.directoryURL = FileManager.default.homeDirectoryForCurrentUser
        if panel.runModal() == .OK, let url = panel.url {
            path = url.path
            if name.trimmingCharacters(in: .whitespaces).isEmpty {
                name = url.lastPathComponent
            }
        }
    }
}
