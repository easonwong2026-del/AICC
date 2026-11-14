import Foundation

/// `tasks.overview`：今天所有平台的任务汇总。
///
/// 三条不能违反的显示规则：
///
/// 1. 数字**全部来自本地记录**（助手侧一次网络请求都不发），所以界面上要写清
///    "只统计本地记录"，避免被当成实时状态；
/// 2. `qoderClaimable` 为 `nil` 表示**还没查过**，界面写"未查询"，绝不写 0；
/// 3. 失败和未完成分开：失败要人处理，未完成只需要等。
public struct TasksOverview: Equatable {
    public let today: String
    public let accountTotal: Int
    public let checkinDone: Int
    public let checkinPending: Int
    public let checkinFailed: Int
    public let growthRuns: Int
    public let growthFailed: Int
    public let travelClaimed: Int
    public let travelPending: Int
    public let qoderClaimed: Int
    public let qoderFailed: Int
    public let qoderAuto: Int
    /// 需要联网才知道，未查询时为 nil。
    public let qoderClaimable: Int?
    public let needsReauth: Int
    public let autoEnabled: Int
    public let failureCount: Int
    public let recentFailures: [TaskFailure]

    public init?(json: [String: Any]) {
        guard let today = json["today"] as? String else { return nil }
        self.today = today
        let accounts = json["accounts"] as? [String: Any]
        self.accountTotal = accounts?["total"] as? Int ?? 0
        let checkin = json["checkin"] as? [String: Any]
        self.checkinDone = checkin?["done"] as? Int ?? 0
        self.checkinPending = checkin?["pending"] as? Int ?? 0
        self.checkinFailed = checkin?["failed"] as? Int ?? 0
        let growth = json["growth"] as? [String: Any]
        self.growthRuns = growth?["runs"] as? Int ?? 0
        self.growthFailed = growth?["failed"] as? Int ?? 0
        let travel = json["travel"] as? [String: Any]
        self.travelClaimed = travel?["claimed"] as? Int ?? 0
        self.travelPending = travel?["pending"] as? Int ?? 0
        let qoder = json["qoderCredits"] as? [String: Any]
        self.qoderClaimed = qoder?["claimed"] as? Int ?? 0
        self.qoderFailed = qoder?["failed"] as? Int ?? 0
        self.qoderAuto = qoder?["autoEnabled"] as? Int ?? 0
        // `?? 0` 会在这里出错：null 是"未查询"，不能变成 0。所以只取 Int。
        self.qoderClaimable = qoder?["claimable"] as? Int
        self.needsReauth = (json["needsReauth"] as? Int) ?? 0
        self.autoEnabled = (json["autoEnabled"] as? Int) ?? 0
        self.failureCount = (json["failureCount"] as? Int) ?? 0
        self.recentFailures = ((json["recentFailures"] as? [[String: Any]]) ?? [])
            .compactMap(TaskFailure.init(json:))
    }

    public var doneToday: Int { checkinDone + travelClaimed + qoderClaimed }
    public var pendingToday: Int { checkinPending + travelPending }
    public var failedToday: Int { checkinFailed + growthFailed + qoderFailed }
}

public struct TaskFailure: Identifiable, Equatable {
    public let platform: String
    public let accountId: String
    public let nickname: String
    public let task: String
    public let status: String
    public let detail: String?
    public let at: String

    public var id: String { "\(platform):\(accountId):\(task):\(at)" }

    public init?(json: [String: Any]) {
        guard let task = json["task"] as? String else { return nil }
        self.platform = (json["platform"] as? String) ?? ""
        self.accountId = (json["accountId"] as? String) ?? ""
        self.nickname = (json["nickname"] as? String) ?? ""
        self.task = task
        self.status = (json["status"] as? String) ?? ""
        self.detail = (json["detail"] as? String).flatMap { $0.isEmpty ? nil : $0 }
        self.at = (json["at"] as? String) ?? ""
    }

    public var taskLabel: String {
        switch task {
        case "checkin": return "签到"
        case "travel": return "旅行"
        case "qoder_credits": return "Qoder Credits"
        default: return task.replacingOccurrences(of: "growth:", with: "成长 ")
        }
    }

    public var displayName: String {
        nickname.isEmpty ? QoderServiceAccount.masked(accountId) : nickname
    }
}
