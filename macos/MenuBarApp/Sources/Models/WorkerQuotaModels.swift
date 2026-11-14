import Foundation

/// 助手侧 `quota.board` 的一行（四平台统一模型）。
///
/// 三个字段的语义是刻意的，界面不许"美化"：
/// - `total/used/remaining` 为 `nil` = **未知**，不是 0；
/// - `unit` 原样带出（`credits` / `tokens` / `nil`），**不同单位不能相加**；
/// - `status == "unverified"` = 这个平台一个请求都没发，原因在 `error` 里；
/// - `status == "stale"` = 这次没取到，显示的是旧快照，`error` 说明原因；
/// - `usageError` 非空但 `usage` 为空 = **余额有效、用量失败**，两件事分开显示。
public struct WorkerQuotaRow: Identifiable, Equatable {
    public let accountId: String
    public let provider: String
    public let nickname: String
    public let source: String
    public let status: String
    public let unit: String?
    public let total: Double?
    public let used: Double?
    public let remaining: Double?
    public let plan: String?
    public let isPaid: Bool?
    public let usage: WorkerQuotaUsage?
    /// 用量单独失败的原因。有额度没用量是正常状态，不是整行失败。
    public let usageError: String?
    public let syncAt: String?
    public let capturedAt: String?
    public let error: String?
    public let cached: Bool
    /// 数据来自旧快照（本次刷新失败）。界面必须标出来，不能当成实时值。
    public let stale: Bool

    public var id: String { "\(provider):\(accountId)" }

    /// 有额度可显示（`ok` 或 `stale`）。`stale` 也是真实数据，只是旧的。
    public var hasQuota: Bool { status == "ok" || status == "stale" }

    public init?(json: [String: Any]) {
        guard let accountId = json["accountId"] as? String,
              let provider = json["provider"] as? String,
              let status = json["status"] as? String
        else { return nil }
        self.accountId = accountId
        self.provider = provider
        self.nickname = (json["nickname"] as? String) ?? ""
        self.source = (json["source"] as? String) ?? "unknown"
        self.status = status
        self.unit = json["unit"] as? String
        self.total = WorkerQuotaRow.number(json["total"])
        self.used = WorkerQuotaRow.number(json["used"])
        self.remaining = WorkerQuotaRow.number(json["remaining"])
        self.plan = json["plan"] as? String
        self.isPaid = json["isPaid"] as? Bool
        self.usage = WorkerQuotaUsage(json: json["usage"] as? [String: Any])
        self.usageError = json["usageError"] as? String
        self.syncAt = json["syncAt"] as? String
        self.capturedAt = json["capturedAt"] as? String
        self.error = json["error"] as? String
        self.cached = (json["cached"] as? Bool) ?? false
        self.stale = (json["stale"] as? Bool) ?? false
    }

    /// JSON 里的数值可能是字符串（服务端返回 `"4597.75000075"`）。
    static func number(_ raw: Any?) -> Double? {
        switch raw {
        case let value as Double: return value
        case let value as Int: return Double(value)
        case let value as NSNumber: return value.doubleValue
        case let value as String: return Double(value)
        default: return nil
        }
    }

    public var providerName: String {
        switch provider {
        case "workbuddy": return "WorkBuddy 国内版"
        case "workbuddy_ai": return "WorkBuddy AI"
        case "qoder_cn": return "Qoder 国内版"
        case "qoder_global": return "Qoder International"
        default: return provider
        }
    }

    public var sourceName: String {
        switch source {
        case "own": return "自有授权"
        case "magpie": return "Magpie 借用"
        case "own_browser": return "浏览器登录"
        default: return source
        }
    }

    /// 未知额度必须是 `nil`，界面显示"未知"而不是 "0"。
    public var remainingText: String? {
        guard let remaining else { return nil }
        return WorkerQuotaRow.format(remaining)
    }

    public var totalText: String? {
        guard let total else { return nil }
        return "总额 " + WorkerQuotaRow.format(total) + (unit.map { " \($0)" } ?? "")
    }

    /// 剩余占比（0…1）。任一未知就返回 nil：不拿未知去画进度条。
    public var remainingShare: Double? {
        guard let remaining, let total, total > 0 else { return nil }
        return max(0, min(100, remaining / total * 100))
    }

    static func format(_ value: Double) -> String {
        if value == value.rounded() && abs(value) < 1e15 {
            return String(format: "%.0f", value)
        }
        return String(format: "%.2f", value)
    }
}

public struct WorkerQuotaUsage: Equatable {
    public let windowDays: Int
    public let requests: Int
    public let credits: Double
    public let truncated: Bool
    public let models: [WorkerQuotaModelUsage]

    public init?(json: [String: Any]?) {
        guard let json else { return nil }
        self.windowDays = (json["windowDays"] as? Int) ?? 0
        self.requests = (json["requests"] as? Int) ?? 0
        self.credits = WorkerQuotaRow.number(json["credits"]) ?? 0
        self.truncated = (json["truncated"] as? Bool) ?? false
        let list = (json["models"] as? [[String: Any]]) ?? []
        self.models = list.compactMap { item in
            guard let model = item["model"] as? String else { return nil }
            return WorkerQuotaModelUsage(
                model: model,
                requests: (item["requests"] as? Int) ?? 0,
                credits: WorkerQuotaRow.number(item["credits"]) ?? 0
            )
        }
    }
}

public struct WorkerQuotaModelUsage: Equatable, Identifiable {
    public let model: String
    public let requests: Int
    public let credits: Double

    public var id: String { model }
}
