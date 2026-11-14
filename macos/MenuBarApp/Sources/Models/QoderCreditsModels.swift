import Foundation

/// Qoder 看板行里的 `credits` 块：**当日奖励**，不是账户余额。
///
/// `state` 的取值决定界面能用哪句话：
///
/// | state | 说明 | 能不能点领取 |
/// | --- | --- | --- |
/// | `claimable` | 今天有可领的活动 | 能 |
/// | `already` | 今天已经领过 | 不能（点了也是 already） |
/// | `nothing` | 窗口内没有领取型活动 | 不能（等服务端下一次窗口） |
/// | `no_credential` | 有账号但没有 AICC 自己的凭据 | 不能（先去授权） |
/// | `unsupported` | 这个版本没有已证实的接口 | 不能（International） |
/// | `unauthorized` | 401/403 | 不能（重新授权） |
/// | `error` | 网络 / 5xx / 结构不认识 | 可以重试 |
public struct QoderCreditsInfo: Equatable {
    public let state: String
    public let claimable: Int?
    public let claimed: Int?
    public let pending: Int?
    public let supported: Bool
    public let todayCredits: Double?
    public let todayCampaignId: String?
    public let todayClaimStatus: String?
    /// 服务端给的活动窗口结束时间（Unix 秒）。**不自己推算刷新点**。
    public let windowEnd: Int?

    public init?(json: [String: Any]?) {
        guard let json, let state = json["state"] as? String else { return nil }
        self.state = state
        self.claimable = json["claimable"] as? Int
        self.claimed = json["claimed"] as? Int
        self.pending = json["pending"] as? Int
        self.supported = (json["supported"] as? Bool) ?? false
        let today = json["today"] as? [String: Any]
        self.todayCredits = WorkerQuotaRow.number(today?["credits"])
        self.todayCampaignId = today?["campaignId"] as? String
        self.todayClaimStatus = today?["claimStatus"] as? String
        self.windowEnd = today?["windowEnd"] as? Int
    }

    /// 一句话状态。未知数字不写 0。
    public var summary: String {
        switch state {
        case "claimable":
            if let todayCredits {
                return "今日可领取 \(WorkerQuotaRow.format(todayCredits)) Credits"
            }
            return "今日有可领取的活动（数量未公布）"
        case "already":
            if let todayCredits {
                return "今日已领取 \(WorkerQuotaRow.format(todayCredits)) Credits"
            }
            return "今日已领取"
        case "nothing":
            return "当前没有可领取的 Credits 活动"
        case "no_credential":
            return "没有可用凭据，先授权再用"
        case "unsupported":
            return "该版本暂未支持（没有已证实的接口）"
        case "unauthorized":
            return "凭据被拒绝，需要重新授权"
        case "busy":
            return "这个账号正在领取，稍后再试"
        default:
            return "查询失败"
        }
    }

    /// 只有真的"今天能领"才让按钮变成可点。其余状态点下去只会得到一个同样的结论。
    public var canClaim: Bool { state == "claimable" }
}

/// `qoder.credits.status` / `qoder.credits.claim` 的返回。
///
/// 两个方法返回同一个形状，所以界面只有一条渲染路径：查完看结果、领完看结果。
public struct QoderCreditsReport: Equatable {
    public let accountId: String
    public let state: String
    public let claimable: Int?
    public let claimed: Bool?
    public let authSource: String?
    public let info: QoderCreditsInfo?
    public let errorMessage: String?
    public let errorKind: String?
    /// 领取成功后服务端回查出来的最新结论（`after`）。没有回查就是 nil，
    /// 界面**不许**用它为 nil 来推断"已经领到了"。
    public let after: QoderCreditsInfo?

    public init(json: [String: Any]) {
        self.accountId = (json["accountId"] as? String) ?? ""
        self.state = (json["state"] as? String) ?? "error"
        self.claimable = json["claimable"] as? Int
        self.claimed = json["claimed"] as? Bool
        self.authSource = json["authSource"] as? String
        // status 的顶层就是 credits 块；claim 的"领完之后"在 after 里。
        self.info = QoderCreditsInfo(json: json)
        self.after = QoderCreditsInfo(json: json["after"] as? [String: Any])
        let error = json["error"] as? [String: Any]
        self.errorMessage = error?["message"] as? String
        self.errorKind = error?["kind"] as? String
    }

    /// 这次调用的结论是否"有数可看"。
    public var isInformative: Bool {
        ["claimable", "already", "nothing", "done"].contains(state)
    }

    /// 查询结论的一句话。未验证不等于失败，文案必须分开。
    public var statusText: String {
        if let info, isInformative { return info.summary }
        if let errorMessage { return errorMessage }
        return "查询完成：\(state)"
    }

    /// 领取结论的一句话。**只有服务端回查确认过才说"已领取"**。
    public var claimText: String {
        if claimed == true {
            if let credits = after?.todayCredits {
                return "已领取 \(WorkerQuotaRow.format(credits)) Credits（服务端已确认）"
            }
            return "已领取（服务端已确认）"
        }
        if state == "busy" { return "这个账号正在领取，请稍后再试" }
        if let errorMessage { return errorMessage }
        if let after { return after.summary }
        return "领取没有完成：\(state)"
    }
}

/// 领取记录（`qoder.credits.runs`）。
public struct QoderCreditRun: Identifiable, Equatable {
    public let id: Int
    public let accountId: String
    public let outcome: String
    public let credits: Double?
    public let authSource: String?
    public let errorKind: String?
    public let errorDetail: String?
    public let trigger: String
    public let executedAt: String

    public init?(json: [String: Any]) {
        guard let id = json["id"] as? Int,
              let outcome = json["outcome"] as? String
        else { return nil }
        self.id = id
        self.accountId = (json["accountId"] as? String) ?? ""
        self.outcome = outcome
        self.credits = WorkerQuotaRow.number(json["credits"])
        self.authSource = json["authSource"] as? String
        self.errorKind = json["errorKind"] as? String
        self.errorDetail = json["errorDetail"] as? String
        self.trigger = (json["trigger"] as? String) ?? "manual"
        self.executedAt = (json["executedAt"] as? String) ?? ""
    }

    public var outcomeLabel: String {
        switch outcome {
        case "ok": return "已领取"
        case "already": return "已领过"
        case "nothing": return "无可领活动"
        case "rejected": return "授权被拒"
        case "forbidden": return "无权限"
        case "rate_limited": return "被限流"
        case "server_error": return "服务端错误"
        case "network": return "网络失败"
        case "unsupported": return "未支持"
        default: return "失败"
        }
    }

    public var isSuccess: Bool { outcome == "ok" || outcome == "already" }
    public var triggerLabel: String { trigger == "auto" ? "自动" : "手动" }
}

/// 自动领取的总览（`qoder.credits.auto.status`）。
public struct QoderCreditsAutoItem: Identifiable, Equatable {
    public let accountId: String
    public let edition: String
    public let nickname: String
    public let autoEnabled: Bool
    public let supported: Bool
    public let lastRun: QoderCreditRun?

    public var id: String { accountId }

    public init?(json: [String: Any]) {
        guard let accountId = json["accountId"] as? String else { return nil }
        self.accountId = accountId
        self.edition = (json["edition"] as? String) ?? ""
        self.nickname = (json["nickname"] as? String) ?? ""
        self.autoEnabled = (json["autoEnabled"] as? Bool) ?? false
        self.supported = (json["supported"] as? Bool) ?? false
        self.lastRun = (json["lastRun"] as? [String: Any]).flatMap(QoderCreditRun.init(json:))
    }

    public var editionName: String {
        switch edition {
        case "qoder_cn": return "Qoder CN"
        case "qoder_global": return "Qoder International"
        default: return edition
        }
    }

    public var displayName: String {
        nickname.isEmpty ? QoderServiceAccount.masked(accountId) : nickname
    }
}
