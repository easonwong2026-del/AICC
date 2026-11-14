import SwiftUI

enum ConsolePage: String, CaseIterable, Identifiable {
    case overview
    case accounts
    case quota
    case checkin

    var id: String { rawValue }

    var title: String {
        switch self {
        case .overview: return "总览"
        case .accounts: return "AI 账号"
        case .quota: return "额度看板"
        case .checkin: return "签到任务"
        }
    }

    /// 一句话说明这一页负责什么。只留一行，不写长段落（PRD §7.5）。
    var subtitle: String {
        switch self {
        case .overview: return "账号、任务与异常"
        case .accounts: return "账号列表、授权与详情"
        case .quota: return "额度与用量"
        case .checkin: return "每日签到与奖励"
        }
    }

    var symbol: String {
        switch self {
        case .overview: return "square.grid.2x2"
        case .accounts: return "person.2"
        case .quota: return "chart.bar.doc.horizontal"
        case .checkin: return "calendar.badge.checkmark"
        }
    }
}

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
    }

    // MARK: 侧边栏

    private var sidebar: some View {
        VStack(alignment: .leading, spacing: 0) {
            // 顶部留出红绿灯的位置（窗口用了全尺寸内容 + 透明标题栏）。
            Color.clear.frame(height: 22)

            HStack(spacing: 9) {
                AppMark()
                VStack(alignment: .leading, spacing: 1) {
                    Text("AICC")
                        .font(.system(size: 14, weight: .semibold))
                        .foregroundStyle(ConsoleTheme.text)
                    Text("AI 工作台")
                        .font(ConsoleType.caption)
                        .foregroundStyle(ConsoleTheme.secondaryText)
                }
                Spacer(minLength: 0)
            }
            .padding(.horizontal, 14)
            .padding(.bottom, 14)

            VStack(spacing: 2) {
                ForEach(ConsolePage.allCases) { item in
                    sidebarRow(item)
                }
            }
            .padding(.horizontal, 8)

            Spacer(minLength: 12)

            statusFooter
                .padding(.horizontal, 14)
                .padding(.bottom, 14)
        }
        .frame(width: ConsoleTheme.sidebarWidth)
        .frame(maxHeight: .infinity, alignment: .top)
        .background(ConsoleTheme.sidebar)
    }

    private func sidebarRow(_ item: ConsolePage) -> some View {
        let selected = nav.page == item
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
            }
            .foregroundStyle(selected ? ConsoleTheme.text : ConsoleTheme.secondaryText)
            .padding(.horizontal, 10)
            .frame(height: 30)
            .background(
                selected ? ConsoleTheme.selectionFill : Color.clear,
                in: RoundedRectangle(cornerRadius: 8)
            )
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .help(item.subtitle)
    }

    /// 系统健康状态固定在侧栏底部，不占主页面。
    private var statusFooter: some View {
        VStack(alignment: .leading, spacing: 6) {
            ConsoleStatusDot(
                color: store.workerHealthColor,
                text: store.workerHealthText
            )
            ConsoleStatusDot(
                color: store.magpieHealthColor,
                text: store.magpieHealthText
            )
        }
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
            VStack(alignment: .leading, spacing: 1) {
                Text(nav.page.title)
                    .font(ConsoleType.pageTitle)
                    .foregroundStyle(ConsoleTheme.text)
                    .lineLimit(1)
                Text(nav.page.subtitle)
                    .font(ConsoleType.pageSubtitle)
                    .foregroundStyle(ConsoleTheme.secondaryText)
                    .lineLimit(1)
            }
            Spacer(minLength: 8)

            if refreshing {
                ProgressView().controlSize(.small)
            }

            Button {
                Task { await refreshCurrentPage() }
            } label: {
                Image(systemName: "arrow.clockwise")
                    .font(.system(size: 11, weight: .medium))
            }
            .buttonStyle(ConsoleIconButtonStyle())
            .help("刷新当前页")
            .disabled(refreshing)

            Button(action: openSettings) {
                Image(systemName: "gearshape")
                    .font(.system(size: 11, weight: .medium))
            }
            .buttonStyle(ConsoleIconButtonStyle())
            .help("设置")
        }
        .padding(.horizontal, ConsoleTheme.pagePadding)
        .frame(height: ConsoleTheme.headerHeight)
        .background(ConsoleTheme.canvas)
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
            async let accounts: Void = store.refreshAccounts()
            async let qoder: Void = store.refreshQoder(silent: true)
            async let quota: Void = store.refreshQuota(force: false)
            _ = await (overview, accounts, qoder, quota)
        case .accounts:
            async let accounts: Void = store.refreshAccounts()
            async let qoder: Void = store.refreshQoder(silent: true)
            _ = await (accounts, qoder)
        case .quota:
            switch nav.quotaSource {
            case .magpie:
                await store.refreshQuota(force: true)
            case .accounts:
                await store.refreshWorkerQuota(force: true)
            }
        case .checkin:
            async let overview: Void = store.refreshTasksOverview()
            async let auto: Void = store.loadCheckinAuto()
            _ = await (overview, auto)
            // 任务状态只在用户点过对应区域的刷新时才联网，这里保持本地缓存。
            await store.loadCheckin(cached: true, silent: true)
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
                        .padding(.bottom, 20)
                }
            } else {
                content
                    .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
            }
        }
    }
}

extension ConsoleStore {
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
