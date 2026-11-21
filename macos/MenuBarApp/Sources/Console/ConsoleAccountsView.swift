import AppKit
import SwiftUI

/// AI 账号：左栏只负责找到账号，右栏负责理解状态与执行操作。
/// 服务账号、客户端登录和本机 Qoder 配置始终是三种不同实体。
struct ConsoleAccountsView: View {
    @ObservedObject var store: ConsoleStore
    @EnvironmentObject private var nav: ConsoleNavigation

    @State private var query = ""
    @State private var activeSheet: ActiveConsoleSheet?
    @State private var verifyNotice: [String: String] = [:]
    @State private var busyAccount: String?
    @State private var renamingProfile: QoderProfile?
    @State private var renameText = ""
    @State private var removingProfile: QoderProfile?
    @State private var showRecoverConfirm = false
    @State private var switchingTo: ConsoleAccount?
    @State private var restorePromptShown = false
    @State private var rotationSheetShown = false
    /// QA 钩子：离屏渲染需要一张“奖励已展开”的真实详情图；生产环境永远默认收起。
    @State private var rewardsExpanded = UserDefaults.standard.bool(forKey: "AICCConsoleExpandRewards")

    private var trimmedQuery: String {
        query.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
    }

    var body: some View { profileAlerts }

    private var baseView: some View {
        VStack(spacing: 0) {
            toolbar
            Divider().overlay(ConsoleTheme.border)
            HStack(spacing: 0) {
                listColumn.frame(width: ConsoleTheme.accountListWidth)
                Divider().overlay(ConsoleTheme.border)
                detailColumn.frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
            }
        }
        .task { await refreshWbClient() }
        .onChange(of: visibleEntries.map(\.selection)) { _, _ in reconcileSelection() }
        .onAppear { reconcileSelection() }
        .onChange(of: nav.openRotationSheet) { _, requested in
            // 总览待处理事项的一次性导航请求：打开既有轮换管理面板，不自动恢复。
            guard requested else { return }
            nav.openRotationSheet = false
            rotationSheetShown = true
        }
    }

    private var sheetPresentation: some View {
        baseView
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
                        reconcileSelection()
                    }
                }
            }
            .sheet(isPresented: $rotationSheetShown) {
                ConsoleWorkbuddyRotationSheet(store: store)
            }
    }

    private var workbuddyAlerts: some View {
        sheetPresentation
            .alert("切换 WorkBuddy 登录账号？", isPresented: Binding(
                get: { switchingTo != nil },
                set: { if !$0 { switchingTo = nil } }
            )) {
                Button("取消", role: .cancel) { switchingTo = nil }
                Button("切换") {
                    guard let account = switchingTo else { return }
                    switchingTo = nil
                    Task { await store.switchWorkbuddy(to: account) }
                }
            } message: {
                Text(switchAlertMessage)
            }
            .alert("恢复原账号？", isPresented: Binding(
                get: { restorePromptShown || store.wbRestoreNeedsConfirm != nil },
                set: { shown in
                    restorePromptShown = shown
                    if !shown { store.cancelWorkbuddyRestore() }
                }
            )) {
                Button("取消", role: .cancel) {}
                Button("恢复原账号") {
                    restorePromptShown = false
                    if store.wbRestoreNeedsConfirm != nil {
                        Task { await store.confirmWorkbuddyRestore() }
                    } else {
                        let id = store.wbClientStatus?.restoreOffer?.id
                        Task { await store.restoreWorkbuddyPrevious(journalId: id) }
                    }
                }
            } message: {
                Text("会用切换前保存的登录态覆盖当前客户端。若你刚刚手动换过账号，请先取消并确认当前登录。")
            }
    }

    private var profileAlerts: some View {
        workbuddyAlerts
            .alert("重命名本机配置", isPresented: Binding(
                get: { renamingProfile != nil },
                set: { if !$0 { renamingProfile = nil } }
            )) {
                TextField("名称", text: $renameText)
                Button("取消", role: .cancel) { renamingProfile = nil }
                Button("保存") {
                    guard let profile = renamingProfile else { return }
                    let name = renameText.trimmingCharacters(in: .whitespacesAndNewlines)
                    renamingProfile = nil
                    guard !name.isEmpty else { return }
                    Task { await store.qoderRename(profile, to: name) }
                }
            } message: {
                Text("只修改 AICC 中显示的名称。")
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
                    Text("从 AICC 移除只删除登记；永久删除会删除快照 \(profile.snapshotDir)，无法撤销。客户端数据目录不会被删除。")
                }
            }
            .alert("手动恢复？", isPresented: $showRecoverConfirm) {
                Button("取消", role: .cancel) {}
                Button("恢复") {
                    showRecoverConfirm = false
                    Task { await store.qoderRecover() }
                }
            } message: {
                Text("AICC 会先确认客户端已完全退出；无法确认时会拒绝恢复，并保持当前登录不变。")
            }
    }

    // MARK: - 工具栏

    private var toolbar: some View {
        HStack(spacing: 10) {
            ConsoleSearchField(placeholder: "搜索账号", text: $query)
                .frame(minWidth: 140, maxWidth: 220)
            ConsoleFilterMenu(
                title: "平台",
                options: ConsolePlatformFilter.accountFilters,
                label: { $0.title },
                selection: Binding(get: { nav.accountsFilter }, set: { nav.accountsFilter = $0 })
            )
            Spacer(minLength: 4)
            Menu {
                Button("WorkBuddy 登录与夜间轮换") { rotationSheetShown = true }
                Button("管理本机 Qoder 配置") { activeSheet = .qoderProfiles(.cn) }
            } label: {
                Image(systemName: "slider.horizontal.3")
            }
            .menuStyle(.borderlessButton)
            .menuIndicator(.hidden)
            .frame(width: ConsoleTheme.controlHeight, height: ConsoleTheme.controlHeight)
            .help("管理")
            Button("添加账号") { activeSheet = .addAccount(ConsoleAddRequest()) }
                .buttonStyle(ConsolePrimaryButtonStyle())
        }
        .padding(.horizontal, 24)
        .padding(.vertical, 12)
    }

    // MARK: - 列表数据

    private struct Entry: Identifiable {
        let selection: ConsoleAccountSelection
        let name: String
        let platform: String
        let status: String
        let tone: ConsoleTheme.Tone
        let symbol: String
        let live: Bool

        var id: ConsoleAccountSelection { selection }
        var subtitle: String {
            tone == .ok || tone == .neutral || status.isEmpty ? platform : "\(platform) · \(status)"
        }
    }

    private struct Section: Identifiable {
        let title: String
        let entries: [Entry]
        var id: String { title }
    }

    private func matches(_ text: String) -> Bool {
        trimmedQuery.isEmpty || text.contains(trimmedQuery)
    }

    private func wbEntry(_ account: ConsoleAccount) -> Entry {
        let isLive = store.wbClientStatus?.live.loggedIn == true && store.wbClientStatus?.live.uid == account.uid
        return Entry(
            selection: .workbuddy(account.id), name: account.nickname,
            platform: account.isWorkbuddyAI ? "WorkBuddy 国际版" : "WorkBuddy 国内版",
            status: account.displayAuthTone == .ok ? "" : account.displayAuthLabel,
            tone: ConsoleTheme.consoleTone(account.displayAuthTone), symbol: account.symbolName, live: isLive
        )
    }

    private func qoderEntry(_ account: QoderServiceAccount) -> Entry {
        let tone: ConsoleTheme.Tone = account.usableSources.isEmpty ? .warning : .ok
        return Entry(
            selection: .qoderService(account.id), name: account.displayName,
            platform: account.edition == "qoder_cn" ? "Qoder 中国版" : "Qoder 国际版",
            status: account.usableSources.isEmpty ? account.statusLabel : "",
            tone: tone, symbol: account.edition == "qoder_cn" ? "chevron.left.forwardslash.chevron.right" : "globe",
            live: false
        )
    }

    private func profileEntry(_ profile: QoderProfile) -> Entry {
        Entry(
            selection: .qoderProfile(profile.id), name: profile.displayName,
            platform: "本机配置 · \(profile.edition == "qoder_cn" ? "中国版" : "国际版")",
            status: profile.authTone == .ok ? "" : profile.authLabel,
            tone: ConsoleTheme.consoleTone(profile.authTone), symbol: profile.symbolName, live: profile.active
        )
    }

    private var sections: [Section] {
        let filter = nav.accountsFilter
        var result: [Section] = []
        let wb = store.accounts.filter { filter.includes($0) && matches($0.searchText) }
        let services = store.qoderServiceAccounts.filter { matches($0.searchText) }
        let profiles = store.qoderProfiles.filter { matches($0.searchText) }

        func append(_ title: String, _ entries: [Entry]) {
            if !entries.isEmpty { result.append(Section(title: title, entries: entries)) }
        }
        if filter.showsWorkbuddyCN {
            append("WorkBuddy 国内版", wb.filter { $0.isWorkbuddy(international: false) }.map(wbEntry))
            append("Qoder 中国版", services.filter { $0.edition == "qoder_cn" }.map(qoderEntry))
        }
        if filter.showsWorkbuddyAI {
            append("WorkBuddy 国际版", wb.filter { $0.isWorkbuddy(international: true) }.map(wbEntry))
        }
        if filter == .all {
            append("Qoder 国际版", services.filter { $0.edition == "qoder_global" }.map(qoderEntry))
        } else if filter == .qoderGlobal {
            append("Qoder 国际版", services.filter { $0.edition == "qoder_global" }.map(qoderEntry))
        }
        if filter.showsQoder {
            if filter == .all || filter == .qoder {
                append("Qoder 本机配置 · 中国版", profiles.filter { $0.edition == "qoder_cn" }.map(profileEntry))
            }
            if filter == .all || filter == .qoderGlobal {
                append("Qoder 本机配置 · 国际版", profiles.filter { $0.edition == "qoder_global" }.map(profileEntry))
            }
        }
        return result
    }

    private var visibleEntries: [Entry] { sections.flatMap(\.entries) }

    private func reconcileSelection() {
        let entries = visibleEntries
        guard !entries.isEmpty else { nav.accountSelection = nil; return }
        if let selected = nav.accountSelection, entries.contains(where: { $0.selection == selected }) { return }
        nav.accountSelection = entries.first?.selection
    }

    private var listColumn: some View {
        VStack(alignment: .leading, spacing: 0) {
            if store.loadingAccounts && visibleEntries.isEmpty && store.accountsError == nil {
                Spacer()
                ConsoleLoadingRow(text: "正在读取账号")
                    .frame(maxWidth: .infinity, alignment: .center)
                Spacer()
            } else if let error = store.accountsError ?? store.qoderError, visibleEntries.isEmpty {
                Spacer()
                ConsoleInlineNotice(kind: .error, message: error, actionTitle: "重试") {
                    Task {
                        await store.refreshAccounts()
                        await store.refreshQoder(silent: false)
                        await refreshWbClient()
                    }
                }
                .padding(.horizontal, 12)
                Spacer()
            } else {
                if trimmedQuery.isEmpty && nav.accountsFilter.showsWorkbuddyCN {
                    clientEntry
                    Divider().overlay(ConsoleTheme.border).padding(.leading, 16)
                }
                ScrollView {
                    LazyVStack(alignment: .leading, spacing: 0, pinnedViews: [.sectionHeaders]) {
                        ForEach(sections) { section in
                            SwiftUI.Section {
                                ForEach(section.entries) { entry in accountRow(entry) }
                            } header: {
                                Text(section.title)
                                    .font(ConsoleType.micro)
                                    .foregroundStyle(ConsoleTheme.tertiaryText)
                                    .padding(.horizontal, 16)
                                    .padding(.top, 12)
                                    .padding(.bottom, 6)
                                    .frame(maxWidth: .infinity, alignment: .leading)
                                    .background(ConsoleTheme.sidebar)
                            }
                        }
                    }
                }
                if let error = store.accountsError ?? store.qoderError {
                    ConsoleInlineNotice(kind: .error, message: error, actionTitle: "重试") {
                        Task {
                            await store.refreshAccounts()
                            await store.refreshQoder(silent: false)
                            await refreshWbClient()
                        }
                    }
                    .padding(12)
                }
                if visibleEntries.isEmpty && store.accountsError == nil && store.qoderError == nil {
                    Spacer()
                    ConsoleEmptyState(
                        symbol: trimmedQuery.isEmpty ? "person.crop.circle.badge.plus" : "magnifyingglass",
                        title: trimmedQuery.isEmpty ? "还没有账号" : "没有匹配结果",
                        message: trimmedQuery.isEmpty ? "添加账号后开始管理授权和任务。" : "换个关键词或平台筛选。",
                        actionTitle: trimmedQuery.isEmpty ? "添加账号" : "清除搜索",
                        action: { if trimmedQuery.isEmpty { activeSheet = .addAccount(ConsoleAddRequest()) } else { query = "" } }
                    )
                    Spacer()
                }
            }
        }
        .background(ConsoleTheme.sidebar)
    }

    /// 左栏顶部的客户端入口：只回答"现在登录的是谁、要不要人处理"。
    ///
    /// 三种状态严格区分：
    /// - 还没读到客户端状态 →「正在读取」，不能说「未登录」；
    /// - 明确 `loggedIn == false` →「未登录」；
    /// - 轮换的 `attentionRun` 存在 → 优先显示恢复异常与「去处理」，点击只打开管理面板。
    private var clientEntry: some View {
        let status = store.wbClientStatus
        let attention = store.wbRotationStatus?.attentionRun
        let current = store.accounts.first { $0.id == status?.liveAccountID }
        let title: String
        if status == nil {
            title = "正在读取"
        } else if status?.live.loggedIn == true {
            title = current?.nickname ?? status?.live.uid.map(WorkbuddyLiveLogin.shortUID) ?? "未识别账号"
        } else {
            title = "未登录"
        }
        let detail = attention?.restoreText
            ?? status?.restoreOffer?.temporaryNote
            ?? (store.wbRotationStatus?.plan.enabled == true ? "夜间轮换已开启" : "登录与夜间轮换")
        let warning = attention != nil || status?.restoreOffer != nil
        return HStack(spacing: 10) {
            Button { rotationSheetShown = true } label: {
                HStack(spacing: 10) {
                    Image(systemName: "desktopcomputer")
                        .font(ConsoleType.body)
                        .foregroundStyle(warning ? ConsoleTheme.warning : ConsoleTheme.secondaryText)
                        .frame(width: 20)
                    VStack(alignment: .leading, spacing: 2) {
                        Text("当前客户端")
                            .font(ConsoleType.micro)
                            .foregroundStyle(ConsoleTheme.tertiaryText)
                        Text(title)
                            .font(ConsoleType.rowTitle)
                            .foregroundStyle(ConsoleTheme.text)
                            .lineLimit(1)
                        Text(detail)
                            .font(ConsoleType.micro)
                            .foregroundStyle(warning ? ConsoleTheme.warning : ConsoleTheme.tertiaryText)
                            .lineLimit(2)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                    Spacer(minLength: 4)
                }
                .padding(.vertical, 12)
                .padding(.trailing, 8)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .help(attention != nil ? "上一轮轮换没有完成恢复，需要人工确认" : "管理 WorkBuddy 登录与夜间轮换")

            if attention != nil {
                Button("去处理") { rotationSheetShown = true }
                    .buttonStyle(ConsoleSecondaryButtonStyle())
                    .help("打开轮换管理面板，按原有确认流程恢复；不会自动覆盖当前登录")
            } else {
                Image(systemName: "chevron.right")
                    .font(ConsoleType.caption)
                    .foregroundStyle(ConsoleTheme.tertiaryText)
                    .padding(.trailing, 16)
            }
        }
        .padding(.leading, 16)
    }

    private func accountRow(_ entry: Entry) -> some View {
        let selected = nav.accountSelection == entry.selection
        return Button { nav.accountSelection = entry.selection } label: {
            HStack(spacing: 10) {
                Image(systemName: entry.symbol)
                    .font(ConsoleType.body)
                    .foregroundStyle(selected ? ConsoleTheme.accent : ConsoleTheme.secondaryText)
                    .frame(width: 20)
                VStack(alignment: .leading, spacing: 3) {
                    HStack(spacing: 6) {
                        Text(entry.name)
                            .font(ConsoleType.rowTitle)
                            .foregroundStyle(ConsoleTheme.text)
                            .lineLimit(1)
                            .truncationMode(.middle)
                        if entry.live {
                            Text("当前")
                                .font(ConsoleType.micro)
                                .foregroundStyle(ConsoleTheme.accent)
                        }
                    }
                    Text(entry.subtitle)
                        .font(ConsoleType.micro)
                        .foregroundStyle(entry.tone == .warning || entry.tone == .error ? ConsoleTheme.color(entry.tone) : ConsoleTheme.secondaryText)
                        .lineLimit(1)
                }
                Spacer(minLength: 4)
                if entry.tone == .warning || entry.tone == .error {
                    Circle().fill(ConsoleTheme.color(entry.tone)).frame(width: 6, height: 6)
                }
            }
            .padding(.horizontal, 14)
            .frame(minHeight: ConsoleTheme.accountRowHeight)
            .background(selected ? ConsoleTheme.selectionFill : Color.clear)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }

    // MARK: - 详情

    @ViewBuilder
    private var detailColumn: some View {
        switch nav.accountSelection {
        case let .workbuddy(id):
            if let account = store.accounts.first(where: { $0.id == id }) {
                workbuddyDetail(account)
            } else { missingSelection }
        case let .qoderService(id):
            if let account = store.qoderServiceAccounts.first(where: { $0.id == id }) {
                qoderDetail(account)
            } else { missingSelection }
        case let .qoderProfile(id):
            if let profile = store.qoderProfiles.first(where: { $0.id == id }) {
                profileDetail(profile)
            } else { missingSelection }
        case nil:
            if store.loadingAccounts {
                VStack { Spacer(); ConsoleLoadingRow(text: "正在读取账号"); Spacer() }.frame(maxWidth: .infinity)
            } else if visibleEntries.isEmpty {
                // 错误与空状态都只在左栏出现一次（含重试入口）；右栏保持安静。
                Color.clear
            } else {
                emptyDetail
            }
        }
    }

    private var emptyDetail: some View {
        VStack {
            Spacer()
            ConsoleEmptyState(
                symbol: "person.crop.circle", title: "选择一个账号",
                message: "查看状态、今日任务与可用操作。"
            )
            Spacer()
        }
    }

    private var missingSelection: some View {
        VStack {
            Spacer()
            ConsoleEmptyState(symbol: "person.crop.circle.badge.questionmark", title: "账号已更新", message: "请在左侧重新选择。")
            Spacer()
        }
    }

    private func detailScroll<Content: View>(@ViewBuilder content: () -> Content) -> some View {
        ScrollView {
            VStack(alignment: .leading, spacing: ConsoleTheme.sectionSpacing) {
                globalNotices
                content()
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.horizontal, ConsoleTheme.pagePadding)
            .padding(.top, ConsoleTheme.pageTopPadding)
            .padding(.bottom, ConsoleTheme.pageBottomPadding)
        }
    }

    @ViewBuilder
    private var globalNotices: some View {
        if let pending = store.qoderPendingSwitches.first {
            ConsoleInlineNotice(
                kind: .warning,
                message: "\(pending.editionName) 有一次切换尚未完成，请确认客户端已退出后再恢复。",
                actionTitle: "恢复",
                action: { showRecoverConfirm = true }
            )
        }
        if let notice = store.qoderNotice {
            ConsoleInlineNotice(kind: .info, message: notice)
        }
        if let notice = store.qoderDeleteNotice {
            ConsoleInlineNotice(kind: .info, message: notice)
        }
        if let error = store.qoderDeleteError {
            ConsoleInlineNotice(kind: .error, message: error)
        }
        if let notice = store.wbSwitchNotice {
            ConsoleInlineNotice(
                kind: store.wbSwitchFailed ? .error : .info,
                message: notice,
                actionTitle: "知道了",
                action: { store.clearWorkbuddySwitchNotice() }
            )
        }
    }

    private func detailHeader(
        name: String, platform: String, symbol: String,
        status: String, tone: ConsoleTheme.Tone, live: Bool = false,
        @ViewBuilder menu: () -> some View
    ) -> some View {
        HStack(alignment: .top, spacing: 12) {
            Image(systemName: symbol)
                .font(.system(size: 18, weight: .medium))
                .foregroundStyle(ConsoleTheme.accent)
                .frame(width: 38, height: 38)
                .background(ConsoleTheme.accentSoft, in: RoundedRectangle(cornerRadius: 10))
            VStack(alignment: .leading, spacing: 4) {
                Text(name)
                    .font(ConsoleType.detailTitle)
                    .foregroundStyle(ConsoleTheme.text)
                    .lineLimit(2)
                    .fixedSize(horizontal: false, vertical: true)
                HStack(spacing: 6) {
                    Text(platform)
                    Text("·")
                    Text(status).foregroundStyle(ConsoleTheme.color(tone))
                }
                .font(ConsoleType.small)
                .foregroundStyle(ConsoleTheme.secondaryText)
                if live { ConsoleBadge(text: "当前登录", tint: ConsoleTheme.accent) }
            }
            Spacer(minLength: 8)
            Menu(content: menu) {
                Image(systemName: "ellipsis")
                    .frame(width: 30, height: 30)
            }
            .menuStyle(.borderlessButton)
            .menuIndicator(.hidden)
            .help("更多操作")
        }
    }

    private func detailMetric(
        _ label: String, _ value: String,
        unit: String? = nil,
        tone: Color = ConsoleTheme.text
    ) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(label).font(ConsoleType.caption).foregroundStyle(ConsoleTheme.secondaryText)
            HStack(alignment: .firstTextBaseline, spacing: 4) {
                Text(value)
                    .font(ConsoleType.rowTitle)
                    .foregroundStyle(tone)
                    .lineLimit(1)
                    .minimumScaleFactor(0.6)
                if let unit {
                    Text(unit)
                        .font(ConsoleType.micro)
                        .foregroundStyle(ConsoleTheme.tertiaryText)
                        .lineLimit(1)
                        .minimumScaleFactor(0.6)
                }
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private func quotaMetric(accountId: String) -> (String, String, String?, Color)? {
        guard let row = store.workerQuotaRows.first(where: { $0.accountId == accountId }) else { return nil }
        if row.stale { return ("额度", "旧数据", nil, ConsoleTheme.warning) }
        guard row.hasQuota, let remaining = row.remainingText else {
            return ("额度", row.error == nil ? "未查询" : "更新失败", nil, ConsoleTheme.warning)
        }
        // 单位用小字跟在数字后面：最小窗口下数字本身不能被截断。
        return ("剩余额度", remaining, row.unit, ConsoleTheme.text)
    }

    // MARK: WorkBuddy 详情

    private func workbuddyDetail(_ account: ConsoleAccount) -> some View {
        let status = store.wbClientStatus
        let candidate = status?.candidate(forAccount: account.id)
        let isLive = status?.live.loggedIn == true && status?.live.uid == account.uid
        return detailScroll {
            detailHeader(
                name: account.nickname,
                platform: account.isWorkbuddyAI ? "WorkBuddy 国际版" : "WorkBuddy 国内版",
                symbol: account.symbolName,
                status: account.displayAuthTone == .ok ? "授权正常" : account.displayAuthLabel,
                tone: ConsoleTheme.consoleTone(account.displayAuthTone), live: isLive
            ) {
                Button("验证授权") { Task { await verify(account) } }
                Button("刷新账号信息") { Task { await store.refreshAccounts() } }
                Button("复制 UID") { copy(account.uid) }
            }
            if let problem = account.authorizationProblem {
                ConsoleInlineNotice(kind: .warning, message: problem)
            }
            if let notice = verifyNotice[account.id] {
                ConsoleInlineNotice(kind: account.displayAuthTone == .error ? .error : .info, message: notice)
            }
            workbuddyActions(account, candidate: candidate, isLive: isLive)
            workbuddyToday(account)
            DisclosureGroup("授权详情") {
                workbuddyAuthorizations(account).padding(.top, ConsoleTheme.groupSpacing)
            }
            .font(ConsoleType.sectionTitle)
            .foregroundStyle(ConsoleTheme.text)
            if account.supportsCNTasks {
                DisclosureGroup(isExpanded: $rewardsExpanded) {
                    rewardsBlock(for: account).padding(.top, ConsoleTheme.groupSpacing)
                } label: {
                    Text("奖励")
                }
                .font(ConsoleType.sectionTitle)
                .foregroundStyle(ConsoleTheme.text)
            }
            DisclosureGroup("账号信息") {
                VStack(alignment: .leading, spacing: 8) {
                    ConsoleInfoRow(label: "UID", value: account.uid, mono: true)
                    ConsoleInfoRow(label: "授权来源", value: account.activeSourceLabel)
                    ConsoleInfoRow(label: "更新时间", value: ConsoleFormat.shortTimestamp(account.updatedAt))
                }
                .padding(.top, ConsoleTheme.groupSpacing)
            }
            .font(ConsoleType.sectionTitle)
            .foregroundStyle(ConsoleTheme.text)
        }
    }

    @ViewBuilder
    private func workbuddyActions(
        _ account: ConsoleAccount, candidate: WorkbuddySwitchCandidate?, isLive: Bool
    ) -> some View {
        ViewThatFits(in: .horizontal) {
            HStack(spacing: 10) { workbuddyActionButtons(account, candidate: candidate, isLive: isLive) }
            VStack(alignment: .leading, spacing: 10) { workbuddyActionButtons(account, candidate: candidate, isLive: isLive) }
        }
        if let candidate, !candidate.switchable, candidate.reason != .live, candidate.reason != .unsupportedProvider {
            Text("暂时无法切换")
                .font(ConsoleType.caption)
                .foregroundStyle(ConsoleTheme.secondaryText)
                .help(candidate.reason.explanation)
        }
        if let note = candidate?.temporaryNote {
            Text(note).font(ConsoleType.caption).foregroundStyle(ConsoleTheme.warning)
        }
        if let live = store.wbClientStatus?.live, isLive, let temporary = live.temporarySummary {
            Text(temporary).font(ConsoleType.caption).foregroundStyle(ConsoleTheme.warning)
        }
        if store.wbClientStatus?.restoreOffer != nil && isLive {
            Text("可以恢复切换前的登录账号")
                .font(ConsoleType.caption)
                .foregroundStyle(ConsoleTheme.warning)
        }
    }

    @ViewBuilder
    private func workbuddyActionButtons(
        _ account: ConsoleAccount, candidate: WorkbuddySwitchCandidate?, isLive: Bool
    ) -> some View {
        if candidate?.switchable == true && !isLive {
            Button(candidate?.mode?.buttonTitle ?? "切换到此账号") { switchingTo = account }
                .buttonStyle(ConsolePrimaryButtonStyle())
                .disabled(store.wbSwitching != nil)
        } else if isLive && store.wbClientStatus?.restoreOffer != nil {
            Button("恢复原账号…") { restorePromptShown = true }
                .buttonStyle(ConsolePrimaryButtonStyle())
                .disabled(store.wbSwitching != nil)
        } else if account.displayAuthTone == .error || account.authorizations.isEmpty {
            Button("重新授权") { activeSheet = .addAccount(ConsoleAddRequest(
                platform: account.isWorkbuddyAI ? .workbuddyAI : .workbuddyCN,
                method: .browser
            )) }
                .buttonStyle(ConsolePrimaryButtonStyle())
        }
        if account.supportsCNTasks {
            Button("查看任务") { nav.openCheckin(platform: .workbuddy) }
                .buttonStyle(ConsoleSecondaryButtonStyle())
        }
        Button("查看额度") {
            nav.quotaSource = .accounts
            nav.quotaProvider = account.provider
            nav.page = .quota
        }
        .buttonStyle(ConsoleSecondaryButtonStyle())
        if store.wbSwitching == account.id || store.wbSwitching == "restore" {
            ProgressView().controlSize(.small)
        }
    }

    private var switchAlertMessage: String {
        guard let account = switchingTo else { return "" }
        let candidate = store.wbClientStatus?.candidate(forAccount: account.id)
        if candidate?.mode == .magpieTemporary {
            return "将关闭并重启 WorkBuddy。这是一份临时登录，\(WorkbuddyTime.remaining(candidate?.remainingSeconds))，到期后客户端会掉线；请在到期前回到 AICC 恢复原账号。重启会中断正在进行的对话；失败会自动恢复原账号。"
        }
        return "将关闭并重启 WorkBuddy，正在进行的对话可能被中断。切换后会读取一次当前登录与活跃状态；失败会自动恢复原账号。"
    }

    @ViewBuilder
    private func workbuddyToday(_ account: ConsoleAccount) -> some View {
        if account.supportsCNTasks {
            let checkin = store.checkin.row(account.id)
            let growth = store.growthResult.scans.first { $0.accountId == account.id }
            let travel = store.travelRows.first { $0.accountId == account.id }?.status
            ConsoleDetailGroup(title: "今日") {
                HStack(spacing: 16) {
                    detailMetric("签到", checkin?.state.title ?? "未查询", tone: checkin?.needsAttention == true ? ConsoleTheme.warning : ConsoleTheme.text)
                    Divider().frame(height: 34)
                    detailMetric("成长", growth.map { $0.pendingCount > 0 ? "\($0.pendingCount) 项待办" : "无待办" } ?? "未查询")
                    Divider().frame(height: 34)
                    detailMetric("旅行", travel?.stageText ?? "未查询", tone: travel?.stage == .failed ? ConsoleTheme.warning : ConsoleTheme.text)
                    if let quota = quotaMetric(accountId: account.id) {
                        Divider().frame(height: 34)
                        detailMetric(quota.0, quota.1, unit: quota.2, tone: quota.3)
                    }
                }
                .frame(minHeight: 44)
            }
        } else if let quota = quotaMetric(accountId: account.id) {
            ConsoleDetailGroup(title: "用量") { detailMetric(quota.0, quota.1, unit: quota.2, tone: quota.3) }
        }
    }

    private func workbuddyAuthorizations(_ account: ConsoleAccount) -> some View {
        VStack(alignment: .leading, spacing: 12) {
            if account.authorizations.isEmpty {
                HStack {
                    Text("需要登录").font(ConsoleType.body).foregroundStyle(ConsoleTheme.warning)
                    Spacer()
                    Button("重新授权") { activeSheet = .addAccount(ConsoleAddRequest(
                platform: account.isWorkbuddyAI ? .workbuddyAI : .workbuddyCN,
                method: .browser
            )) }
                        .buttonStyle(ConsoleSecondaryButtonStyle())
                }
            }
            if let own = account.ownAuthorization {
                HStack(alignment: .firstTextBaseline, spacing: 10) {
                    Text("自有授权").font(ConsoleType.rowTitle).foregroundStyle(ConsoleTheme.text)
                    Text(own.authLabel).font(ConsoleType.small).foregroundStyle(own.status == "valid" ? ConsoleTheme.secondaryText : ConsoleTheme.warning)
                    if let expires = own.tokenExpiresAt { Text("有效至 \(ConsoleFormat.shortTimestamp(expires))").font(ConsoleType.caption).foregroundStyle(ConsoleTheme.tertiaryText) }
                    Spacer(minLength: 4)
                    if busyAccount == account.id { ProgressView().controlSize(.mini) }
                    Button("验证") { Task { await verify(account) } }.buttonStyle(.borderless).font(ConsoleType.small)
                }
            }
            if let magpie = account.magpieAuthorization {
                HStack(alignment: .firstTextBaseline, spacing: 10) {
                    Text("Magpie").font(ConsoleType.rowTitle).foregroundStyle(ConsoleTheme.text)
                    Text(magpie.authLabel).font(ConsoleType.small).foregroundStyle(ConsoleTheme.secondaryText)
                    Spacer()
                    Text(account.isBorrowed ? "由 Magpie 管理" : "已保存访问令牌")
                        .font(ConsoleType.caption).foregroundStyle(ConsoleTheme.tertiaryText)
                        .help("Magpie 借用由 Magpie 刷新；其他访问令牌不在这里自动续期。")
                }
            }
        }
    }

    private func verify(_ account: ConsoleAccount) async {
        busyAccount = account.id
        defer { busyAccount = nil }
        verifyNotice[account.id] = await store.verifyWorkbuddyAccount(account)
    }

    private func rewardsBlock(for account: ConsoleAccount) -> ConsoleRewardsBlock {
        ConsoleRewardsBlock(
            state: store.rewardStates[account.id],
            loading: store.rewardLoading.contains(account.id),
            failure: store.rewardFailures[account.id],
            result: store.rewardActions[account.id],
            acting: store.rewardActing.contains(account.id),
            preview: store.rewardPreviews[account.id],
            checking: store.rewardChecking.contains(account.id),
            onRefresh: { Task { await store.loadReward(accountId: account.id) } },
            onPreview: { action, target in
                Task { await store.previewReward(accountId: account.id, action: action, target: target) }
            },
            onConfirmPreview: {
                guard let preview = store.rewardPreviews[account.id],
                      let action = RewardAction(rawValue: preview.action) else { return }
                store.clearRewardPreview(accountId: account.id)
                Task { await store.performReward(accountId: account.id, action: action, target: preview.target) }
            },
            onCancelPreview: { store.clearRewardPreview(accountId: account.id) },
            onRecheck: { Task { await store.recheckRewards(accountId: account.id) } },
            onDismissResult: { store.clearRewardAction(accountId: account.id) }
        )
    }

    // MARK: Qoder 服务账号详情

    private func qoderDetail(_ account: QoderServiceAccount) -> some View {
        let edition = QoderEdition(rawValue: account.edition) ?? .cn
        let healthy = !account.usableSources.isEmpty
        return detailScroll {
            detailHeader(
                name: account.displayName,
                platform: account.edition == "qoder_cn" ? "Qoder 中国版" : "Qoder 国际版",
                symbol: account.edition == "qoder_cn" ? "chevron.left.forwardslash.chevron.right" : "globe",
                status: healthy ? "授权正常" : account.statusLabel,
                tone: healthy ? .ok : .warning
            ) {
                Button("刷新账号信息") { Task { await store.refreshQoder(silent: false) } }
                Button("检测本机 PAT") { Task { await checkPat(account) } }
                if account.hasPAT {
                    Button("移除 PAT") { Task { await removePat(account) } }
                }
                Divider()
                Button("删除账号") { activeSheet = .deleteQoderAccount(account.id) }
            }
            ViewThatFits(in: .horizontal) {
                HStack(spacing: 10) { qoderActions(account, edition: edition) }
                VStack(alignment: .leading, spacing: 10) { qoderActions(account, edition: edition) }
            }
            ConsoleDetailGroup(title: "今日") {
                HStack(spacing: 16) {
                    let run = store.qoderCreditsAuto.first { $0.accountId == account.id }?.lastRun
                    detailMetric("每日 Credits", run?.outcomeLabel ?? "未查询", tone: run?.isSuccess == false ? ConsoleTheme.warning : ConsoleTheme.text)
                    if let quota = quotaMetric(accountId: account.id) {
                        Divider().frame(height: 34)
                        detailMetric(quota.0, quota.1, unit: quota.2, tone: quota.3)
                    }
                }
            }
            DisclosureGroup("授权详情") {
                VStack(alignment: .leading, spacing: 12) {
                    ForEach(account.authorizations, id: \.source) { authorization in
                        HStack(spacing: 10) {
                            Text(authorization.sourceLabel).font(ConsoleType.rowTitle).foregroundStyle(ConsoleTheme.text)
                            Text(authorization.statusLabel).font(ConsoleType.small).foregroundStyle(authorization.authStatus == "valid" ? ConsoleTheme.secondaryText : ConsoleTheme.warning)
                            Spacer()
                            Text(authorization.hasSecret ? "已保存凭据" : (authorization.source == "magpie" ? "由 Magpie 管理" : "未保存凭据"))
                                .font(ConsoleType.caption).foregroundStyle(ConsoleTheme.tertiaryText)
                        }
                    }
                    if account.authorizations.isEmpty {
                        Text("需要登录").font(ConsoleType.body).foregroundStyle(ConsoleTheme.warning)
                    }
                    HStack(spacing: 10) {
                        Button("浏览器授权") { activeSheet = .addAccount(ConsoleAddRequest(platform: edition == .cn ? .qoderCN : .qoderGlobal, method: .browser)) }
                            .buttonStyle(ConsoleSecondaryButtonStyle())
                        Button("添加 PAT") { activeSheet = .addAccount(ConsoleAddRequest(platform: edition == .cn ? .qoderCN : .qoderGlobal, method: .pat)) }
                            .buttonStyle(ConsoleSecondaryButtonStyle())
                    }
                }
                .padding(.top, ConsoleTheme.groupSpacing)
            }
            .font(ConsoleType.sectionTitle)
            .foregroundStyle(ConsoleTheme.text)
            DisclosureGroup("账号信息") {
                VStack(alignment: .leading, spacing: 8) {
                    ConsoleInfoRow(label: "UID", value: account.uid, mono: true)
                    ConsoleInfoRow(label: "邮箱", value: account.email ?? "—")
                    ConsoleInfoRow(label: "版本", value: edition.displayName)
                }
                .padding(.top, ConsoleTheme.groupSpacing)
            }
            .font(ConsoleType.sectionTitle)
            .foregroundStyle(ConsoleTheme.text)
        }
    }

    @ViewBuilder
    private func qoderActions(_ account: QoderServiceAccount, edition: QoderEdition) -> some View {
        if account.usableSources.isEmpty {
            Button("浏览器授权") { activeSheet = .addAccount(ConsoleAddRequest(platform: edition == .cn ? .qoderCN : .qoderGlobal, method: .browser)) }
                .buttonStyle(ConsolePrimaryButtonStyle())
        }
        if account.usableSources.isEmpty {
            Button("查看每日 Credits") { nav.openCheckin(platform: .qoder) }
                .buttonStyle(ConsoleSecondaryButtonStyle())
        } else {
            Button("查看每日 Credits") { nav.openCheckin(platform: .qoder) }
                .buttonStyle(ConsolePrimaryButtonStyle())
        }
        Button("查看额度") {
            nav.quotaSource = .accounts
            nav.quotaProvider = account.edition
            nav.page = .quota
        }
        .buttonStyle(ConsoleSecondaryButtonStyle())
    }

    // MARK: 本机 Qoder 配置详情

    private func profileDetail(_ profile: QoderProfile) -> some View {
        let busy = store.qoderActing == profile.id
        return detailScroll {
            detailHeader(
                name: profile.displayName,
                platform: "本机配置 · \(profile.edition == "qoder_cn" ? "中国版" : "国际版")",
                symbol: profile.symbolName,
                status: profile.authLabel,
                tone: ConsoleTheme.consoleTone(profile.authTone), live: profile.active
            ) {
                Button("重命名") {
                    renameText = profile.displayName
                    renamingProfile = profile
                }
                Button("检测认证文件") { Task { await store.qoderInspect(profile) } }
                Divider()
                Button("移除配置", role: .destructive) { removingProfile = profile }
                    .disabled(profile.runtime.isRunning)
            }
            if profile.runtime.isUnknown {
                ConsoleInlineNotice(kind: .warning, message: "客户端进程归属无法确认，AICC 不会对它执行关闭或恢复。")
            }
            if profile.snapshotIncomplete {
                ConsoleInlineNotice(kind: .warning, message: "登录快照可能不完整，请重新保存当前登录。")
            }
            ConsoleDetailGroup(title: "客户端") {
                HStack(spacing: 10) {
                    detailMetric("运行状态", profile.runtimeLabel, tone: ConsoleTheme.tone(profile.runtimeTone))
                    Spacer(minLength: 8)
                    if busy { ProgressView().controlSize(.small) }
                    Button(profile.runtime.isRunning ? "关闭客户端" : "打开客户端") {
                        Task {
                            if profile.runtime.isRunning { await store.qoderClose(profile) }
                            else { await store.qoderOpen(profile) }
                        }
                    }
                    .buttonStyle(ConsolePrimaryButtonStyle())
                    .disabled(busy)
                    Button("保存当前登录") { Task { await store.qoderCapture(profile) } }
                        .buttonStyle(ConsoleSecondaryButtonStyle())
                        .disabled(busy)
                }
            }
            DisclosureGroup("配置详情") {
                VStack(alignment: .leading, spacing: 8) {
                    ConsoleInfoRow(label: "快照", value: profile.snapshotReady ? "已保存" : "尚未保存", valueTint: profile.snapshotReady ? ConsoleTheme.text : ConsoleTheme.warning)
                    ConsoleInfoRow(label: profile.identityLabel, value: profile.identityText ?? "—", mono: profile.accountUid != nil)
                    ConsoleInfoRow(label: "客户端", value: profile.clientVersion.map { "\(profile.editionName) \($0)" } ?? profile.editionName)
                    ConsoleInfoRow(label: "Bundle ID", value: profile.appBundleId ?? "—", mono: true)
                    ConsoleInfoRow(label: "认证指纹", value: profile.fingerprintShort, mono: true)
                    ConsoleInfoRow(label: "快照目录", value: profile.snapshotDir, mono: true)
                    ConsoleInfoRow(label: "客户端目录", value: profile.liveDir, mono: true)
                    Button("检测认证文件") { Task { await store.qoderInspect(profile) } }
                        .buttonStyle(ConsoleSecondaryButtonStyle())
                }
                .padding(.top, ConsoleTheme.groupSpacing)
            }
            .font(ConsoleType.sectionTitle)
            .foregroundStyle(ConsoleTheme.text)
        }
    }

    // MARK: - 公共动作

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
            let removal = try await store.worker.removeQoderPat(accountId: account.id)
            store.showQoderNotice(removal.message)
        } catch {
            store.showQoderNotice((error as? LocalizedError)?.errorDescription ?? error.localizedDescription)
        }
        await store.refreshQoder(silent: true)
    }

    private func copy(_ text: String) {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(text, forType: .string)
    }

    private func refreshWbClient() async {
        await store.refreshWorkbuddyClient(silent: true)
        await store.refreshWorkbuddyRotation(silent: true)
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
        var title: String { self == .create ? "新建配置" : "导入已有目录" }
    }

    private var info: QoderEditionInfo? { store.qoderEditions.first { $0.edition == edition.rawValue } }
    private var canSubmit: Bool {
        mode == .create || (path.hasPrefix("/") && !path.trimmingCharacters(in: .whitespaces).isEmpty)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            HStack(spacing: 10) {
                Image(systemName: "internaldrive")
                    .foregroundStyle(ConsoleTheme.accent)
                    .frame(width: 32, height: 32)
                    .background(ConsoleTheme.accentSoft, in: RoundedRectangle(cornerRadius: 8))
                VStack(alignment: .leading, spacing: 2) {
                    Text("本机 Qoder 配置").font(ConsoleType.sectionTitle).foregroundStyle(ConsoleTheme.text)
                    Text("保存本机登录，不切换账号").font(ConsoleType.caption).foregroundStyle(ConsoleTheme.secondaryText)
                }
            }
            Picker("版本", selection: $edition) {
                ForEach(QoderEdition.allCases) { Text($0.displayName).tag($0) }
            }
            .pickerStyle(.segmented).labelsHidden()
            if let info {
                Label("\(info.bundleId) · \(info.installSummary)", systemImage: info.installed ? "checkmark.circle" : "exclamationmark.triangle")
                    .font(ConsoleType.caption)
                    .foregroundStyle(info.installed ? ConsoleTheme.secondaryText : ConsoleTheme.warning)
                    .lineLimit(1)
            }
            Picker("方式", selection: $mode) {
                ForEach(Mode.allCases) { Text($0.title).tag($0) }
            }
            .pickerStyle(.segmented).labelsHidden()
            VStack(alignment: .leading, spacing: 6) {
                Text("显示名称").font(ConsoleType.caption).foregroundStyle(ConsoleTheme.secondaryText)
                TextField(edition.defaultProfileName, text: $name).textFieldStyle(.roundedBorder)
            }
            if mode == .importExisting {
                VStack(alignment: .leading, spacing: 6) {
                    Text("已有账号快照目录（需包含 auth.v1.dat）").font(ConsoleType.caption).foregroundStyle(ConsoleTheme.secondaryText)
                    HStack {
                        TextField("选择目录", text: $path).textFieldStyle(.roundedBorder).font(ConsoleType.mono)
                        Button("选择…") { pickDirectory() }.buttonStyle(ConsoleSecondaryButtonStyle())
                    }
                    Text("只登记目录，不移动或覆盖文件。").font(ConsoleType.caption).foregroundStyle(ConsoleTheme.tertiaryText)
                }
            } else {
                Text("创建后请打开 \(edition.displayName) 登录，再回来点「保存当前登录」。")
                    .font(ConsoleType.caption).foregroundStyle(ConsoleTheme.secondaryText)
                    .fixedSize(horizontal: false, vertical: true)
            }
            Divider().overlay(ConsoleTheme.border)
            HStack {
                Button("取消") { dismiss() }.buttonStyle(ConsoleSecondaryButtonStyle())
                Spacer()
                Button(mode == .create ? "创建" : "导入") {
                    let finalName = name.trimmingCharacters(in: .whitespacesAndNewlines)
                    let displayName = finalName.isEmpty ? edition.defaultProfileName : finalName
                    let importPath = path.trimmingCharacters(in: .whitespacesAndNewlines)
                    let importMode = mode == .importExisting
                    let targetEdition = edition
                    dismiss()
                    Task {
                        if importMode { await store.importQoderProfile(edition: targetEdition, snapshotDir: importPath, displayName: displayName) }
                        else { await store.addQoderProfile(edition: targetEdition, displayName: displayName) }
                    }
                }
                .buttonStyle(ConsolePrimaryButtonStyle())
                .disabled(!canSubmit)
            }
        }
        .padding(ConsoleTheme.Sheet.padding)
        .frame(width: 460)
        .background(ConsoleTheme.canvas)
        .onAppear { edition = initialEdition; name = initialEdition.defaultProfileName }
    }

    private func pickDirectory() {
        let panel = NSOpenPanel()
        panel.canChooseDirectories = true
        panel.canChooseFiles = false
        panel.allowsMultipleSelection = false
        panel.directoryURL = FileManager.default.homeDirectoryForCurrentUser
        if panel.runModal() == .OK, let url = panel.url {
            path = url.path
            if name.trimmingCharacters(in: .whitespaces).isEmpty { name = url.lastPathComponent }
        }
    }
}
