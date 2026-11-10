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

    var subtitle: String {
        switch self {
        case .overview: return "账号、额度与运行状态一屏看完"
        case .accounts: return "管理 WorkBuddy 与 Qoder CN 账号"
        case .quota: return "Magpie 各服务商额度与重置时间"
        case .checkin: return "每日签到与成长任务"
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

/// AICC 主控制台：左侧导航 + 右侧内容，一个窗口装下账号、额度、签到。
struct MainConsoleView: View {
    @ObservedObject var store: ConsoleStore
    let openSettings: () -> Void

    @State private var page: ConsolePage

    init(store: ConsoleStore, openSettings: @escaping () -> Void) {
        self.store = store
        self.openSettings = openSettings
        // QA 钩子：截图脚本用 AICC_CONSOLE_PAGE 指定初始分页，正常启动不带这个变量。
        let requested = ProcessInfo.processInfo.environment["AICC_CONSOLE_PAGE"]
        _page = State(initialValue: ConsolePage(rawValue: requested ?? "") ?? .overview)
    }

    var body: some View {
        HStack(spacing: 0) {
            sidebar
            content
        }
        .background(ConsoleTheme.canvas)
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
                        .font(.system(size: 11))
                        .foregroundStyle(ConsoleTheme.secondaryText)
                }
                Spacer(minLength: 0)
            }
            .padding(.horizontal, 14)
            .padding(.bottom, 16)

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
        let selected = page == item
        return Button {
            page = item
        } label: {
            HStack(spacing: 9) {
                Image(systemName: item.symbol)
                    .font(.system(size: 12, weight: .medium))
                    .frame(width: 17)
                Text(item.title).font(.system(size: 13, weight: selected ? .semibold : .regular))
                Spacer(minLength: 0)
            }
            .foregroundStyle(selected ? ConsoleTheme.text : ConsoleTheme.secondaryText)
            .padding(.horizontal, 10)
            .frame(height: 32)
            .background(
                selected ? ConsoleTheme.selectionFill : Color.clear,
                in: RoundedRectangle(cornerRadius: 8)
            )
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .help(item.subtitle)
    }

    private var statusFooter: some View {
        VStack(alignment: .leading, spacing: 7) {
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

    // MARK: 内容

    private var content: some View {
        VStack(alignment: .leading, spacing: 0) {
            header
            Divider().overlay(ConsoleTheme.border)
            ScrollView {
                pageBody
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(ConsoleTheme.pagePadding)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .background(ConsoleTheme.canvas)
    }

    private var header: some View {
        HStack(alignment: .firstTextBaseline, spacing: 12) {
            VStack(alignment: .leading, spacing: 3) {
                Text(page.title)
                    .font(.system(size: 19, weight: .semibold))
                    .foregroundStyle(ConsoleTheme.text)
                Text(page.subtitle)
                    .font(.system(size: 11))
                    .foregroundStyle(ConsoleTheme.secondaryText)
            }
            Spacer(minLength: 8)

            Button {
                Task { await refreshCurrentPage() }
            } label: {
                Image(systemName: "arrow.clockwise")
                    .font(.system(size: 12, weight: .medium))
            }
            .buttonStyle(ConsoleIconButtonStyle())
            .help("刷新当前页")
            .disabled(isRefreshing)

            Button(action: openSettings) {
                Image(systemName: "gearshape")
                    .font(.system(size: 12, weight: .medium))
            }
            .buttonStyle(ConsoleIconButtonStyle())
            .help("设置")
        }
        .padding(.horizontal, ConsoleTheme.pagePadding)
        .padding(.top, 20)
        .padding(.bottom, 16)
    }

    @ViewBuilder
    private var pageBody: some View {
        switch page {
        case .overview:
            ConsoleOverviewView(store: store) { target in page = target }
        case .accounts:
            ConsoleAccountsView(store: store)
        case .quota:
            ConsoleQuotaView(store: store)
        case .checkin:
            ConsoleCheckinView(store: store)
        }
    }

    private var isRefreshing: Bool {
        store.loadingAccounts || store.loadingQuota
    }

    private func refreshCurrentPage() async {
        switch page {
        case .overview, .quota:
            await store.refreshAll()
        case .accounts, .checkin:
            await store.refreshAccounts()
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

struct ConsoleIconButtonStyle: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .foregroundStyle(ConsoleTheme.secondaryText)
            .frame(width: 28, height: 24)
            .background(
                configuration.isPressed ? ConsoleTheme.cardSubtle : ConsoleTheme.card,
                in: RoundedRectangle(cornerRadius: 7)
            )
            .overlay(
                RoundedRectangle(cornerRadius: 7).strokeBorder(ConsoleTheme.border, lineWidth: 1)
            )
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
