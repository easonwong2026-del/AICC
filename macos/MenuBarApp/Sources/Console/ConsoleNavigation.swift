import Foundation
import SwiftUI

/// 主控制台的**页面内导航上下文**（PRD §9.2）。
///
/// 只放"界面看什么"，不放业务数据：账号、额度、任务的真实状态仍在 `ConsoleStore`。
/// 放在这里的原因：切页不能丢筛选和选中项，总览点卡片要能带着平台跳过去。
@MainActor
final class ConsoleNavigation: ObservableObject {
    @Published var page: ConsolePage

    /// AI 账号页的平台筛选；总览卡片点击后会改写它。
    @Published var accountsFilter: ConsolePlatformFilter = .all
    /// 账号页当前选中的账号。切页后保留，异步刷新不会覆盖用户的选择。
    @Published var accountSelection: ConsoleAccountSelection?

    /// 额度页的来源：Magpie 额度 / 账号额度。
    @Published var quotaSource: ConsoleQuotaSource = .magpie
    /// 额度页的服务商筛选（Magpie 来源用）。
    @Published var quotaProvider: String = ""

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

    /// 从额度页跳去签到任务页的某个任务，不做任何写操作。
    func openCheckin(platform: ConsoleTaskPlatform, workbuddy: WorkbuddyTaskKind = .checkin, qoder: QoderTaskKind = .credits) {
        checkinPlatform = platform
        workbuddyTask = workbuddy
        qoderTask = qoder
        page = .checkin
    }
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
