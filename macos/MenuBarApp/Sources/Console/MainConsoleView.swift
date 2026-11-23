import SwiftUI

/// AICC 主控制台：左侧固定导航 + 右侧工作区（标题栏 + 页面工具栏 + 内容）。
///
/// 布局约定（PRD §2.3）：
/// - 标题、设置、刷新**永远可见**，不随内容滚动；
/// - 每个页面自己提供工具栏，全局只保留一个刷新与齿轮；
/// - 账号页需要双栏铺满高度，所以内容区不再套统一 ScrollView。
struct MainConsoleView: View {
    @ObservedObject var store: ConsoleStore
    let openSettings: () -> Void

    @StateObject private var nav: ConsoleNavigation
    @State private var refreshing = false
    @State private var hoveredPage: ConsolePage?

    init(store: ConsoleStore, openSettings: @escaping () -> Void) {
        self.store = store
        self.openSettings = openSettings
        // QA 钩子：截图/验收脚本用 `open --args -AICCConsolePage accounts` 指定初始分页。
        // 同时兼容环境变量，方便直接跑二进制。
        let requested = UserDefaults.standard.string(forKey: "AICCConsolePage")
            ?? ProcessInfo.processInfo.environment["AICC_CONSOLE_PAGE"]
        _nav = StateObject(
            wrappedValue: ConsoleNavigation(page: ConsolePage(rawValue: requested ?? "") ?? .overview)
        )
    }

    var body: some View {
        HStack(spacing: 0) {
            sidebar
            workArea
        }
        .background(ConsoleTheme.canvas)
        .environmentObject(nav)
        .frame(
            minWidth: ConsoleTheme.minWindowSize.width,
            minHeight: ConsoleTheme.minWindowSize.height
        )
        .task { await store.loadIfNeeded() }
        .onAppear { store.setActiveConsolePage(nav.page) }
        .onChange(of: nav.page) { _, page in
            store.setActiveConsolePage(page)
        }
    }

    // MARK: 侧边栏

    private var sidebar: some View {
        VStack(alignment: .leading, spacing: 0) {
            // 顶部留出红绿灯的位置（窗口用了全尺寸内容 + 透明标题栏）。
            Color.clear.frame(height: 22)

            HStack(spacing: 10) {
                AppMark()
                Text("AICC")
                    .font(.system(size: 16, weight: .semibold))
                    .foregroundStyle(ConsoleTheme.text)
                Spacer(minLength: 0)
            }
            .padding(.horizontal, 16)
            .padding(.bottom, 24)

            VStack(spacing: 2) {
                ForEach(Array(ConsolePage.allCases.enumerated()), id: \.element.id) { index, item in
                    sidebarRow(item, shortcutIndex: index + 1)
                }
            }
            .padding(.horizontal, 8)

            Spacer(minLength: 12)

            if let connectionNotice = store.connectionNotice {
                ConsoleInlineNotice(
                    kind: .warning,
                    message: connectionNotice,
                    actionTitle: "设置",
                    action: openSettings
                )
                .padding(.horizontal, 12)
                .padding(.bottom, 16)
            }
        }
        .frame(width: ConsoleTheme.sidebarWidth)
        .frame(maxHeight: .infinity, alignment: .top)
        .background(ConsoleTheme.sidebar)
    }

    private func sidebarBadgeCount(for item: ConsolePage) -> Int {
        switch item {
        case .accounts:
            return store.fleet.issues
        case .checkin:
            return store.tasksOverview?.failureCount ?? 0
        case .overview, .quota:
            return 0
        }
    }

    private func sidebarRow(_ item: ConsolePage, shortcutIndex: Int) -> some View {
        let selected = nav.page == item
        let hovered = hoveredPage == item
        let badgeCount = sidebarBadgeCount(for: item)
        return Button {
            nav.page = item
        } label: {
            HStack(spacing: 9) {
                Image(systemName: item.symbol)
                    .font(.system(size: 12, weight: .medium))
                    .frame(width: 17)
                Text(item.title)
                    .font(.system(size: 13, weight: selected ? .semibold : .regular))
                Spacer(minLength: 0)
                if badgeCount > 0 {
                    Text("\(badgeCount)")
                        .font(ConsoleType.micro)
                        .monospacedDigit()
                        .foregroundStyle(ConsoleTheme.warning)
                        .padding(.horizontal, 6)
                        .padding(.vertical, 1)
                        .background(ConsoleTheme.warningSoft, in: Capsule())
                }
            }
            .foregroundStyle(selected ? ConsoleTheme.text : ConsoleTheme.secondaryText)
            .padding(.horizontal, 10)
            .frame(height: 30)
            .background(
                selected
                    ? ConsoleTheme.selectionFill
                    : (hovered ? ConsoleTheme.cardSubtle.opacity(0.85) : Color.clear),
                in: RoundedRectangle(cornerRadius: 8)
            )
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .keyboardShortcut(KeyEquivalent(Character("\(shortcutIndex)")), modifiers: .command)
        .onHover { inside in
            hoveredPage = inside ? item : (hoveredPage == item ? nil : hoveredPage)
        }
        .help("\(item.subtitle)（⌘\(shortcutIndex)）")
    }

    // MARK: 工作区

    private var workArea: some View {
        VStack(spacing: 0) {
            header
            Divider().overlay(ConsoleTheme.border)
            pageBody
                .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .background(ConsoleTheme.canvas)
    }

    private var header: some View {
        HStack(alignment: .center, spacing: 10) {
            Text(nav.page.title)
                .font(ConsoleType.pageTitle)
                .foregroundStyle(ConsoleTheme.text)
                .lineLimit(1)
            Spacer(minLength: 12)

            if refreshing || (nav.page.supportsAccountsAutoRefresh && store.loadingAccounts) {
                ProgressView().controlSize(.small)
            } else if let status = headerAccountsStatus {
                Text(status.text)
                    .font(ConsoleType.micro)
                    .monospacedDigit()
                    .foregroundStyle(status.tone == .warning ? ConsoleTheme.warning : ConsoleTheme.tertiaryText)
                    .lineLimit(1)
                    .help(status.help)
            }

            Button {
                Task { await refreshCurrentPage() }
            } label: {
                Image(systemName: "arrow.clockwise")
                    .font(.system(size: 13, weight: .medium))
            }
            .buttonStyle(ConsoleIconButtonStyle())
            .keyboardShortcut("r", modifiers: .command)
            // 四个页面的刷新语义不同（额度=重新采集，任务=重读本地），
            // 提示必须跟着页面走，否则又会被当成一个"什么都刷"的按钮。
            .help("\(nav.page.refreshHint)（⌘R）")
            .disabled(refreshing)

            Button(action: openSettings) {
                Image(systemName: "gearshape")
                    .font(.system(size: 13, weight: .medium))
            }
            .buttonStyle(ConsoleIconButtonStyle())
            .keyboardShortcut(",", modifiers: .command)
            .help("设置（⌘,）")
        }
        .padding(.horizontal, ConsoleTheme.pagePadding)
        .frame(height: ConsoleTheme.headerHeight)
        .background(ConsoleTheme.canvas)
    }

    private var headerAccountsStatus: ConsoleAccountsHeaderStatus? {
        ConsoleAccountsHeaderStatus.make(
            page: nav.page,
            lastSuccessAt: store.lastAccountsRefresh,
            error: store.accountsError,
            formatTime: ConsoleFormat.shortTime
        )
    }

    @ViewBuilder
    private var pageBody: some View {
        switch nav.page {
        case .overview:
            ConsoleOverviewView(store: store)
        case .accounts:
            ConsoleAccountsView(store: store)
        case .quota:
            ConsoleQuotaView(store: store)
        case .checkin:
            ConsoleCheckinView(store: store)
        }
    }

    // MARK: 刷新

    /// 每页只刷新自己需要的数据（PRD §7.4）。
    ///
    /// 所有调用都是**只读**的：切页或点刷新不会签发、领取、派发任何东西。
    private func refreshCurrentPage() async {
        guard !refreshing else { return }
        refreshing = true
        defer { refreshing = false }
        switch nav.page {
        case .overview:
            async let overview: Void = store.refreshTasksOverview()
            async let accounts: Void = { _ = await store.refreshAccounts(trigger: .manual) }()
            async let qoder: Void = store.refreshQoder(silent: true)
            async let quota: Void = store.refreshQuota(force: false)
            async let rotation: Void = store.refreshWorkbuddyRotation(silent: true)
            _ = await (overview, accounts, qoder, quota, rotation)
        case .accounts:
            async let accounts: Void = { _ = await store.refreshAccounts(trigger: .manual) }()
            async let qoder: Void = store.refreshQoder(silent: true)
            _ = await (accounts, qoder)
        case .quota:
            // 额度页只有这一个刷新入口；具体读哪条链路由 `QuotaRefreshPlan` 决定，
            // 与额度视图里的来源选择共用同一份映射。
            switch QuotaRefreshPlan.pageRefresh(source: nav.quotaSource) {
            case .reloadMagpie:
                await store.refreshQuota(force: true)
            case .reloadAccounts:
                await store.refreshWorkerQuota(force: true)
            case .changeWindow:
                break
            }
        case .checkin:
            // 只刷新当前任务域的本地状态，全部只读；联网检查由页面上的「检查状态/扫描」触发。
            switch nav.checkinPlatform {
            case .workbuddy:
                switch nav.workbuddyTask {
                case .checkin:
                    await store.loadCheckinAuto()
                    await store.loadCheckin(cached: true, silent: true)
                case .growth:
                    await store.loadGrowthAuto()
                case .travel:
                    await store.loadTravel()
                }
            case .qoder:
                await store.refreshQoderCredits(silent: true)
            }
        }
    }
}

/// 侧边栏左上角的标记：用 SF Symbol 拼一个方块底，不引入位图资源。
struct AppMark: View {
    var body: some View {
        Image(systemName: "square.stack.3d.up.fill")
            .font(.system(size: 14, weight: .semibold))
            .foregroundStyle(ConsoleTheme.accent)
            .frame(width: 28, height: 28)
            .background(ConsoleTheme.accentSoft, in: RoundedRectangle(cornerRadius: 8))
    }
}

/// 页面骨架：工具栏固定，内容按需滚动。
struct ConsolePageScaffold<Toolbar: View, Content: View>: View {
    var scrolls: Bool = true
    @ViewBuilder var toolbar: Toolbar
    @ViewBuilder var content: Content

    var body: some View {
        VStack(spacing: 0) {
            toolbar
                .padding(.horizontal, ConsoleTheme.pagePadding)
                .padding(.top, 12)
                .padding(.bottom, 10)
            if scrolls {
                ScrollView {
                    content
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(.horizontal, ConsoleTheme.pagePadding)
                        .padding(.bottom, ConsoleTheme.pageBottomPadding)
                }
            } else {
                content
                    .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
            }
        }
    }
}

extension ConsoleStore {
    /// 正常连接不占界面；只有用户需要处理时才出现。
    var connectionNotice: String? {
        #if AICC_VISUAL_TEST && AICC_CONSOLE_VISUAL
        return nil
        #else
        if case .failed = worker.state { return "账号服务连接失败" }
        if case .running(let health) = worker.state, !health.isHealthy { return "账号服务需要检查" }
        if let snapshot, !snapshot.available { return "Magpie 连接不可用" }
        return nil
        #endif
    }

    var workerHealthText: String {
        if case .running(let health) = worker.state {
            return health.isHealthy ? "账号助手正常" : "账号助手降级"
        }
        return "账号助手未就绪"
    }

    var workerHealthColor: Color {
        if case .running(let health) = worker.state {
            return health.isHealthy ? ConsoleTheme.success : ConsoleTheme.warning
        }
        return ConsoleTheme.tertiaryText
    }

    var magpieHealthText: String {
        guard let snapshot else { return "Magpie 未检查" }
        if !snapshot.available { return "Magpie 不可用" }
        return snapshot.stale ? "Magpie 缓存中" : "Magpie 正常"
    }

    var magpieHealthColor: Color {
        guard let snapshot else { return ConsoleTheme.tertiaryText }
        if !snapshot.available { return ConsoleTheme.danger }
        return snapshot.stale ? ConsoleTheme.warning : ConsoleTheme.success
    }
}
