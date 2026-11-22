import Foundation
import SwiftUI

/// 主控制台的一级页面。属于导航状态，不依赖任何视图。
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
        case .checkin: return "任务中心"
        }
    }

    /// 一句话说明这一页负责什么。只出现在 help 中，不占页头。
    var subtitle: String {
        switch self {
        case .overview: return "账号、任务与异常"
        case .accounts: return "账号列表、授权与详情"
        case .quota: return "额度与用量"
        case .checkin: return "今日任务与自动计划"
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

    /// 页头那个刷新图标这一下到底做什么。
    ///
    /// 四个页面的语义不同：额度是重新采集，任务是重读本地记录。
    /// 说清楚它，才不会和页面里的「检查状态 / 扫描」被当成同一件事。
    var refreshHint: String {
        switch self {
        case .overview:
            return "刷新概览：重读本地汇总，额度优先用快照"
        case .accounts:
            return "刷新账号：向账号助手重读账号与 Qoder 状态"
        case .quota:
            return "刷新额度：向数据源重新读一次（只读，不会领取）"
        case .checkin:
            return "重读本地状态（不联网）；联网检查用页面里的按钮"
        }
    }
}

/// 主控制台的**页面内导航上下文**（PRD §9.2）。
///
/// 只放"界面看什么"，不放业务数据：账号、额度、任务的真实状态仍在 `ConsoleStore`。
/// 放在这里的原因：切页不能丢筛选和选中项，总览点卡片要能带着平台跳过去。
///
/// 放在 Models 而不是 Console 目录：它不含任何 View，却承载跨页导航的状态机，
/// 需要进 `swift test` 覆盖"一次性请求"的消费语义。
@MainActor
final class ConsoleNavigation: ObservableObject {
    @Published var page: ConsolePage

    /// AI 账号页的平台筛选；总览卡片点击后会改写它。
    @Published var accountsFilter: ConsolePlatformFilter = .all
    /// 账号页当前选中的账号。切页后保留，异步刷新不会覆盖用户的选择。
    @Published var accountSelection: ConsoleAccountSelection?

    /// 请求打开「自动轮换」管理界面的一次性信号。
    /// 只承载导航意图，不承载业务状态；消费后立即复位。
    @Published private(set) var rotationRequest: RotationManagerRequest?

    /// 额度页的来源：Magpie 额度 / 账号额度。
    @Published var quotaSource: ConsoleQuotaSource = .magpie
    /// 额度页的服务商筛选（Magpie 来源用）。
    @Published var quotaProvider: String = ""

    /// 任务中心：今日任务 / 自动计划。
    @Published var taskCenterMode: TaskCenterMode = .today

    /// 签到任务页：先选平台，再选任务类型。
    @Published var checkinPlatform: ConsoleTaskPlatform = .workbuddy
    @Published var workbuddyTask: WorkbuddyTaskKind = .checkin
    @Published var qoderTask: QoderTaskKind = .credits

    init(page: ConsolePage) {
        self.page = page
        // QA 钩子（PRD §9.3）：截图 / 验收脚本可以用启动参数直接落到某个页面、
        // 某个账号、某个额度来源或某个任务类型，方便 CI 与人眼验收。
        let defaults = UserDefaults.standard
        if let raw = defaults.string(forKey: "AICCConsoleAccount"),
           let selection = ConsoleAccountSelection(qaValue: raw) {
            accountSelection = selection
        }
        if let raw = defaults.string(forKey: "AICCConsoleQuotaSource"),
           let source = ConsoleQuotaSource(rawValue: raw) {
            quotaSource = source
        }
        if let raw = defaults.string(forKey: "AICCConsoleQuotaProvider"), !raw.isEmpty {
            quotaProvider = raw
        }
        if let raw = defaults.string(forKey: "AICCConsoleTaskCenterMode"),
           let mode = TaskCenterMode(rawValue: raw) {
            taskCenterMode = mode
        }
        if let raw = defaults.string(forKey: "AICCConsoleCheckinPlatform"),
           let platform = ConsoleTaskPlatform(rawValue: raw) {
            checkinPlatform = platform
        }
        if let raw = defaults.string(forKey: "AICCConsoleWorkbuddyTask"),
           let task = WorkbuddyTaskKind(rawValue: raw) {
            workbuddyTask = task
        }
    }

    /// 从总览跳到账号页并筛到某个平台。
    func openAccounts(filter: ConsolePlatformFilter, selection: ConsoleAccountSelection? = nil) {
        accountsFilter = filter
        if let selection {
            accountSelection = selection
        } else if let current = accountSelection, !current.matches(filter: accountsFilter) {
            accountSelection = nil
        }
        page = .accounts
    }

    /// 从额度页跳去任务中心的某个任务，不做任何写操作。
    func openCheckin(
        platform: ConsoleTaskPlatform,
        workbuddy: WorkbuddyTaskKind = .checkin,
        qoder: QoderTaskKind = .credits,
        mode: TaskCenterMode = .today
    ) {
        checkinPlatform = platform
        workbuddyTask = workbuddy
        qoderTask = qoder
        taskCenterMode = mode
        page = .checkin
    }

    /// 切换额度来源。
    ///
    /// 只改"渲染哪张表"，不发任何请求、不改用量窗口：来源只是两条互不相加的链路，
    /// 不是统计口径。服务商筛选按来源各有一套，切来源时一并复位，避免带着
    /// 另一个来源的 provider 显示空表。
    func selectQuotaSource(_ source: ConsoleQuotaSource) {
        guard quotaSource != source else { return }
        quotaSource = source
        quotaProvider = ""
    }

    /// 请求打开自动轮换管理界面。只设置导航意图，不执行任何恢复。
    func requestRotationManager(_ focus: RotationManagerFocus = .plan) {
        rotationRequest = RotationManagerRequest(focus: focus, token: UUID())
    }

    /// 消费一次"打开轮换管理"的请求。
    ///
    /// 两个入口（总览、账号页、任务中心）都走这一条：
    /// - 首次挂载时请求已经存在也能消费；
    /// - 同一次请求只返回一次，不会重复弹窗。
    @discardableResult
    func consumeRotationManagerRequest() -> RotationManagerRequest? {
        guard let request = rotationRequest else { return nil }
        rotationRequest = nil
        return request
    }
}

/// 打开自动轮换管理界面时希望落在哪一段。
enum RotationManagerFocus: String, Equatable {
    /// 计划总览。
    case plan
    /// 执行记录。
    case history
    /// 恢复异常：打开后先看到「需要恢复」与处理入口。
    case recovery
    /// 直接进入设置流程。
    case setup
}

/// 一次性导航请求。`token` 让同一次请求可以被视图的 `onChange` 稳定识别。
struct RotationManagerRequest: Equatable, Identifiable {
    let focus: RotationManagerFocus
    let token: UUID

    var id: UUID { token }
}

/// 额度页的来源。两个数据链路的量纲不同，不合并、不相加。
enum ConsoleQuotaSource: String, CaseIterable, Identifiable {
    case magpie
    case accounts

    var id: String { rawValue }

    var title: String {
        switch self {
        case .magpie: return "Magpie 额度"
        case .accounts: return "账号额度"
        }
    }

    var subtitle: String {
        switch self {
        case .magpie: return "Magpie 采集的额度窗口与重置时间"
        case .accounts: return "助手直连官方接口的账号余额与用量"
        }
    }
}

/// 签到任务页的平台（只有真正有任务的两个）。
enum ConsoleTaskPlatform: String, CaseIterable, Identifiable {
    case workbuddy
    case qoder

    var id: String { rawValue }

    var title: String {
        switch self {
        case .workbuddy: return "WorkBuddy CN"
        case .qoder: return "Qoder CN"
        }
    }

    var symbol: String {
        switch self {
        case .workbuddy: return "bubble.left.and.bubble.right.fill"
        case .qoder: return "chevron.left.forwardslash.chevron.right"
        }
    }
}

enum WorkbuddyTaskKind: String, CaseIterable, Identifiable {
    case checkin
    case growth
    case travel

    var id: String { rawValue }

    var title: String {
        switch self {
        case .checkin: return "每日签到"
        case .growth: return "成长中心"
        case .travel: return "Buddy 旅行"
        }
    }

    var symbol: String {
        switch self {
        case .checkin: return "calendar.badge.checkmark"
        case .growth: return "sparkles"
        case .travel: return "airplane"
        }
    }

    var summary: String {
        switch self {
        case .checkin: return "查询今日状态并领取，每个账号独立开关"
        case .growth: return "任务、积分与连签，可自动接单与领奖"
        case .travel: return "派出、到达与奖励领取"
        }
    }
}

enum QoderTaskKind: String, CaseIterable, Identifiable {
    case credits

    var id: String { rawValue }

    var title: String { "每日 Credits" }
    var symbol: String { "circle.dotted.circle" }
    var summary: String { "按服务端活动窗口领取" }
}
